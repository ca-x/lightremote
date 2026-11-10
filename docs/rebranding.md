# LightRemote → roc-remote

The repository is now `https://github.com/ca-x/roc-remote`. Update a local checkout:

```sh
git remote set-url origin git@github.com:ca-x/roc-remote.git
```

All product, Cargo package, executable, archive and container names now use
`roc-remote`. Rust imports use `roc_remote`, `roc_remote_core`,
`roc_remote_services` and `roc_remote_relay`. The Web and Cloudflare packages
use version `0.1.0`, matching all five Rust workspace packages.

Replace the `LIGHTREMOTE_` environment-variable prefix with `ROC_REMOTE_` in
service definitions, operator secrets, CLI invocations and Cloudflare configuration.
For example, use `ROC_REMOTE_RELAY_TOKEN`, `ROC_REMOTE_INVITE` and
`ROC_REMOTE_WEB_DIR`. Old environment variable names are not aliases. Missing
operator credentials still fail closed. The new relay image is
`ghcr.io/ca-x/roc-remote-relay:0.1.0`.

Only product branding is changing: the `lightremote.*.v1` application profiles,
`lightremote.v1` WebSocket subprotocol, `lightremote.hello` handshake,
`lightremote/v1/…` cryptographic domain separation and canonical test vectors
are unchanged. `.lightremote-upload-` remains the reserved staging prefix so
existing staging files are not made visible by a cosmetic rename. TMP is unchanged.

The project owner explicitly authorized deleting old public Releases and tags to
restart under the new name. Commit history and historical validation documents are
retained. New release tags remain immutable after publication; this is a one-time
branding reset, not permission to overwrite future releases.

The supplied logo and its generated variants are documented in
`assets/branding/README.md`. This release is unsigned, with no real-desktop visual
acceptance or signing/notarization implied by hosted build success.
