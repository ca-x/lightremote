# Deploying the browser client

The browser build is a plain static directory. Build it once:

```sh
cd web
npm ci
npm run build
```

Publish `web/dist/` on a trusted HTTPS static host. It does not need a server-side
session database or an operator token. The invitation fragment supplies the relay
address and single-use controller credentials at connection time. Keep history,
analytics and error collectors from capturing invitation fields.

For a self-hosted relay trusted to deliver client code, set
`ROC_REMOTE_WEB_DIR=/absolute/path/to/web/dist`; the Docker image does this by
default. Put the relay behind a TLS reverse proxy, set its public origin and
allowed client origins, and pass that HTTPS client URL to `roc-remote host
--web-url`. Same-origin hosting trusts the relay operator with client delivery.

For Cloudflare, the Worker in `cloudflare/` serves only the relay API. Deploy the
static client separately, for example using Pages from the repository root:

```sh
cd cloudflare
npx wrangler pages deploy ../web/dist --project-name roc-remote-client
```

The command above mutates your Cloudflare account when you run it; local builds
and tests do not. Set the relay's `ROC_REMOTE_ALLOWED_ORIGINS` to the resulting
client origin and pass the client HTTPS URL to `--web-url`. The client and relay
may use different domains/accounts.

When using a third-party relay, choose an independently trusted client host.
A relay that can replace the application's JavaScript can steal the invitation
secret; storing it in the fragment does not protect against modified client code.
Avoid third-party scripts in the client deployment. Native and CLI controllers
use the installed Rust implementation and do not download executable client code
from the relay.
