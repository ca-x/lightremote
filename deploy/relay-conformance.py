#!/usr/bin/env python3
"""The same dependency-free black-box contract for Rust and Wrangler/Cloudflare.

LIGHTREMOTE_RELAY_TOKEN=test python3 deploy/relay-conformance.py http://127.0.0.1:8787
Uses real RFC 6455 masked binary frames, not an in-process relay mock.
"""
import argparse
import base64
import hashlib
import http.client
import json
import os
import socket
import ssl
import struct
import time
import urllib.parse

MAX_RECORD = 150000


class Relay:
    def __init__(self, base):
        self.url = urllib.parse.urlsplit(base)
        assert self.url.scheme in ('http', 'https')
        assert not self.url.query and not self.url.fragment
        self.token = os.environ['LIGHTREMOTE_RELAY_TOKEN']

    def http(self, method, path, auth=None, origin=None):
        kind = http.client.HTTPSConnection if self.url.scheme == 'https' else http.client.HTTPConnection
        conn = kind(self.url.hostname, self.url.port, timeout=5)
        headers = {'Content-Type': 'application/json'}
        if auth is not None:
            headers['Authorization'] = 'Bearer ' + auth
        if origin is not None:
            headers['Origin'] = origin
        conn.request(method, path, body='{}' if method == 'POST' else None, headers=headers)
        response = conn.getresponse()
        status = response.status
        data = response.read()
        conn.close()
        return status, json.loads(data) if data else None

    def room(self):
        status, room = self.http('POST', '/v1/rooms', self.token)
        assert status == 200, status
        assert len(room['host_token']) == len(room['client_token']) == 43
        assert room['host_token'] != room['client_token']
        assert isinstance(room['expires_at'], int)
        assert time.time() < room['expires_at'] <= time.time() + 86400
        return room

    def ws(self, room, role, ticket=None, query=''):
        ticket = ticket if ticket is not None else room[role + '_token']
        port = self.url.port or (443 if self.url.scheme == 'https' else 80)
        raw = socket.create_connection((self.url.hostname, port), timeout=5)
        if self.url.scheme == 'https':
            raw = ssl.create_default_context().wrap_socket(raw, server_hostname=self.url.hostname)
        key = base64.b64encode(os.urandom(16)).decode()
        # No query credential fallback: supplied query must be rejected by the server.
        request = (f'GET /v1/rooms/{room["room_id"]}/ws{query} HTTP/1.1\r\n'
                   f'Host: {self.url.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                   f'Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n'
                   f'Sec-WebSocket-Protocol: lightremote.v1, {role}.{ticket}\r\n\r\n')
        raw.sendall(request.encode())
        header = bytearray()
        while not header.endswith(b'\r\n\r\n'):
            chunk = raw.recv(1)
            assert chunk, 'handshake closed early'
            header.extend(chunk)
            assert len(header) <= 16384
        lines = header.decode().split('\r\n')
        status = int(lines[0].split()[1])
        if status != 101:
            raw.close()
            return status, None
        headers = {key.lower(): value for line in lines[1:] if ':' in line for key, value in [line.split(':', 1)]}
        assert headers['sec-websocket-protocol'].strip() == 'lightremote.v1'
        expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        assert headers['sec-websocket-accept'].strip() == expected
        return status, WS(raw)

    def join(self, room, role):
        status, ws = self.ws(room, role)
        assert status == 101, status
        return ws


class WS:
    def __init__(self, raw):
        self.raw = raw

    def send(self, body, opcode=2, final=True):
        mask = os.urandom(4)
        size = len(body)
        first = bytes([(128 if final else 0) | opcode])
        if size < 126:
            header = first + bytes([128 | size])
        elif size <= 65535:
            header = first + b'\xfe' + struct.pack('!H', size)
        else:
            header = first + b'\xff' + struct.pack('!Q', size)
        self.raw.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(body)))

    def read(self, length):
        data = bytearray()
        while len(data) < length:
            part = self.raw.recv(length - len(data))
            if not part:
                raise EOFError('socket closed')
            data.extend(part)
        return bytes(data)

    def receive(self):
        while True:
            first, second = self.read(2)
            assert first & 128, 'server fragmentation is unsupported by this small harness'
            assert not second & 128, 'server must not mask frames'
            size = second & 127
            if size == 126:
                size = struct.unpack('!H', self.read(2))[0]
            elif size == 127:
                size = struct.unpack('!Q', self.read(8))[0]
            assert size <= MAX_RECORD + 125
            payload = self.read(size)
            opcode = first & 15
            if opcode == 9:
                self.send(payload, opcode=10)
                continue
            if opcode == 10:
                continue
            return opcode, payload

    def ended(self):
        try:
            assert self.receive()[0] == 8, 'expected close frame'
            self.send(b'', opcode=8)
        except (EOFError, ConnectionResetError, BrokenPipeError):
            pass
        finally:
            self.raw.close()

    def close(self):
        try:
            self.send(b'', opcode=8)
        finally:
            self.raw.close()


def run(base, wait_seconds=None):
    relay = Relay(base)
    status, health = relay.http('GET', '/healthz')
    assert status == 200 and health == {'ok': True, 'protocol': 'lightremote-relay.v1'}
    assert relay.http('POST', '/v1/rooms')[0] == 401
    assert relay.http('POST', '/v1/rooms', 'wrong')[0] == 401
    assert relay.http('GET', '/healthz', origin='https://evil.invalid')[0] == 403
    room = relay.room()
    assert relay.ws(room, 'host', room['client_token'])[0] == 401
    assert relay.ws(room, 'host', query='?token=forbidden')[0] == 400
    host = relay.join(room, 'host')
    assert relay.ws(room, 'host')[0] == 409
    host.send(b'binary pre-peer hello')
    peer = relay.join(room, 'client')
    assert peer.receive() == (2, b'binary pre-peer hello')
    peer.send(b'x' * MAX_RECORD)
    assert host.receive() == (2, b'x' * MAX_RECORD)
    # The complete-message bound also applies to fragmented incoming records.
    host.send(b'a' * 75000, final=False)
    host.send(b'b' * 75000, opcode=0)
    assert peer.receive() == (2, b'a' * 75000 + b'b' * 75000)
    host.close()
    peer.ended()
    assert relay.ws(room, 'client')[0] == 404
    print('PASS auth / role isolation / duplicate / pre-peer / binary forwarding / teardown')
    for body, opcode, final in [(b'o' * (MAX_RECORD + 1), 2, True), (b'text', 1, True)]:
        room = relay.room()
        host, peer = relay.join(room, 'host'), relay.join(room, 'client')
        host.send(body, opcode=opcode, final=final)
        host.ended()
        peer.ended()
        assert relay.ws(room, 'host')[0] == 404
    # Two valid-sized fragments whose complete message is oversized.
    room = relay.room()
    host, peer = relay.join(room, 'host'), relay.join(room, 'client')
    host.send(b'x' * 75000, final=False)
    host.send(b'x' * 75001, opcode=0)
    host.ended()
    peer.ended()
    assert relay.ws(room, 'host')[0] == 404
    print('PASS oversized / fragmented oversize / text rejection')
    room = relay.room()
    host = relay.join(room, 'host')
    for i in range(9):
        host.send(bytes([i]))
    host.ended()
    assert relay.ws(room, 'client')[0] == 404
    print('PASS bounded pre-peer queue overflow')
    if wait_seconds is not None:
        # These scenarios must run against an isolated, short-lived test server.
        # No request is made between joining and receiving a timer-induced close.
        room = relay.room()
        host = relay.join(room, 'host')
        started = time.monotonic()
        host.raw.settimeout(wait_seconds + 3)
        host.ended()
        assert time.monotonic() - started >= wait_seconds - 0.25, 'waiting socket closed before its deadline'
        assert relay.ws(room, 'client')[0] == 404
        print('PASS real waiting timeout / consumed ticket')
        room = relay.room()
        host, peer = relay.join(room, 'host'), relay.join(room, 'client')
        remaining = room['expires_at'] - time.time()
        assert 0 < remaining <= 10, 'expiry test requires room TTL at most 10 seconds'
        host.raw.settimeout(remaining + 3)
        peer.raw.settimeout(remaining + 3)
        host.ended()
        peer.ended()
        assert time.time() >= room['expires_at'], 'paired sockets closed before absolute expiry'
        assert relay.ws(room, 'host')[0] == 404
        assert relay.ws(room, 'client')[0] == 404
        print('PASS real absolute expiry / both idle sockets closed / consumed tickets')
    print('Shared relay conformance passed.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base', help='http(s)://relay')
    parser.add_argument('--wait-seconds', type=float, help='Also test real waiting and expiry timers; requires matching wait config and room TTL <=10 seconds')
    args = parser.parse_args()
    if args.wait_seconds is not None and args.wait_seconds <= 0:
        parser.error('--wait-seconds must be positive')
    run(args.base.rstrip('/'), args.wait_seconds)
