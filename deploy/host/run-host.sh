#!/usr/bin/env bash
# Local policy comes from systemd EnvironmentFile or launchd EnvironmentVariables.
set -euo pipefail
umask 077
ulimit -c 0
unset LIGHTREMOTE_INVITE RUST_LOG RUST_BACKTRACE
fail() { printf '%s\n' 'LightRemote host: invalid local policy; consult resident-host.md.' >&2; exit 64; }

[[ ${LIGHTREMOTE_POLICY_CONFIRMED:-no} == yes ]] || fail
[[ -n ${LIGHTREMOTE_RELAY_TOKEN:-} && ${LIGHTREMOTE_RELAY_TOKEN} != REPLACE_WITH_OPERATOR_TOKEN ]] || fail
[[ ${LIGHTREMOTE_RELAY_URL:-} == https://* ]] || fail
[[ ${LIGHTREMOTE_WEB_URL:-} == https://* ]] || fail
binary=${LIGHTREMOTE_BINARY:-"$HOME/.local/bin/lightremote"}
state=${LIGHTREMOTE_STATE_DIR:-"$HOME/.local/state/lightremote-host"}
[[ $binary == /* && -f $binary && -x $binary ]] || fail
[[ $state == /* && ! -L $state ]] || fail
mkdir -p -- "$state"
[[ -d $state && -O $state && ! -L $state ]] || fail
chmod 700 -- "$state"
[[ ! -L $state/invite && ( ! -e $state/invite || -O $state/invite ) ]] || fail

args=(host --stay --unattended --invite-file "$state/invite"
    --relay "$LIGHTREMOTE_RELAY_URL" --web-url "$LIGHTREMOTE_WEB_URL")
share=${LIGHTREMOTE_SHARE_DIR:-}
if [[ -n $share ]]; then
    [[ $share == /* && -d $share ]] || fail
    args+=(--share-dir "$share")
fi
for option in SHELL WRITE DESKTOP INPUT CLIPBOARD CLIPBOARD_READ CLIPBOARD_WRITE; do
    name=LIGHTREMOTE_ALLOW_$option
    value=${!name:-0}
    [[ $value == 0 || $value == 1 ]] || fail
    if [[ $value == 1 ]]; then
        [[ $option != WRITE || -n $share ]] || fail
        flag=$(printf '%s' "$option" | tr '[:upper:]_' '[:lower:]-')
        args+=("--allow-$flag")
    fi
done
[[ ${LIGHTREMOTE_AUTO_UPDATE:-0} == 0 || ${LIGHTREMOTE_AUTO_UPDATE:-0} == 1 ]] || fail
if [[ ${LIGHTREMOTE_AUTO_UPDATE:-0} == 1 ]]; then args+=(--auto-update); fi

# Only this fixed status reaches the manager. Never forward CLI output or env.
printf '%s\n' 'LightRemote host: starting with confirmed local policy.' >&2
# exec preserves SIGTERM delivery and the CLI exit status (75 requests restart).
exec "$binary" "${args[@]}" </dev/null >/dev/null 2>&1
