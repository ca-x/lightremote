import subprocess,os,tempfile,time,socket,json,pathlib,hashlib
root=str(pathlib.Path(__file__).resolve().parent.parent)
bin=root+'/target/debug/'
with tempfile.TemporaryDirectory() as share,tempfile.TemporaryDirectory() as local:
 s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
 env=dict(os.environ,LIGHTREMOTE_RELAY_TOKEN='smoke-local-operator',LIGHTREMOTE_BIND=f'127.0.0.1:{port}')
 relay=subprocess.Popen([bin+'lightremote-relay'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
 try:
  import urllib.request
  for _ in range(100):
   try:urllib.request.urlopen(f'http://127.0.0.1:{port}/healthz',timeout=1);break
   except:time.sleep(.05)
  def run(action,input=None):
   host=subprocess.Popen([bin+'lightremote','host','--unattended','--relay',f'http://127.0.0.1:{port}','--share-dir',share,'--allow-write','--allow-shell'],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
   try:
    invite=host.stdout.readline().strip().removeprefix('INVITE ')
    assert invite.startswith('http'),host.stderr.read()
    client=subprocess.run([bin+'lightremote','connect',*action],env=dict(env,LIGHTREMOTE_INVITE=invite),input=input,capture_output=True,timeout=20)
    return client
   finally:
    host.terminate()
    try:host.communicate(timeout=3)
    except:host.kill();host.communicate()
  cap=run(['capabilities']);assert cap.returncode==0,cap.stderr
  caps=json.loads(cap.stdout);assert caps['shell'] and caps['files'] and not caps['desktop'];print('PASS headless capabilities')
  source=pathlib.Path(local)/'source';payload=bytes(range(256))*8192;source.write_bytes(payload)
  put=run(['put',str(source),'transfer.bin']);assert put.returncode==0,put.stderr
  receipt=json.loads(put.stdout);assert receipt['sha256']==hashlib.sha256(payload).hexdigest();assert pathlib.Path(share,'transfer.bin').read_bytes()==payload;print('PASS 2MiB atomic upload and sha256 receipt')
  output=pathlib.Path(local)/'received';get=run(['get','transfer.bin',str(output)]);assert get.returncode==0,get.stderr;assert output.read_bytes()==payload;print('PASS 2MiB download')
  missing=pathlib.Path(local)/'missing';get=run(['get','does-not-exist',str(missing)]);assert get.returncode!=0,(get.stdout,get.stderr);assert not missing.exists();print('PASS missing download fails and removes partial file')
  shell=run(['shell'],b"stty -echo\nsleep 0.1; printf '%s_%s_%s\\n' LIGHTREMOTE PTY OK\nexit\n");assert shell.returncode==0,shell.stderr;assert b'LIGHTREMOTE_PTY_OK' in shell.stdout;print('PASS real PTY shell')
 finally:
  relay.terminate();relay.communicate(timeout=3)
