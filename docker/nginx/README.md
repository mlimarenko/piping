# HTTPS

`./install.sh --hostname app.example.com --https-port 8443`
configures a fixed HTTPS redirect authority. `--hostname` accepts a DNS name or
IPv4 address, not a URL; the default is `localhost`. Direct Compose operators
set `PIPING_PUBLIC_HOST` and `PIPING_HTTPS_PORT` in their environment. Redirects
never use the request's Host header. Port 443 is omitted from the redirect.

The host installer owns initial certificate provisioning through host OpenSSL.
If neither TLS file exists, it creates a self-signed certificate with a SAN for
the configured hostname, valid for 365 days. Browsers do not trust this
certificate automatically. For a trusted identity, provision the operator's
certificate chain at `docker/nginx/ssl/fullchain.pem` and unencrypted private
key at `docker/nginx/ssl/privkey.pem` before installation.

Existing TLS files are not rotated. An incomplete pair, expired or malformed
certificate, encrypted or invalid key, mismatched pair or wrong hostname
fails installation. Restore or renew the pair explicitly, then
rerun the installer with the same hostname and ports. Reverting source changes
does not remove or replace the certificate, private key or `.env` secrets.

Stock nginx does not install packages or generate certificates. Its entrypoint
requires both files and runs `nginx -t` before startup. Its Compose healthcheck
requests `/health` over TLS; it accepts self-signed certificates but requires a
successful handshake and HTTP response. Installer deployment waits up to 180
seconds for Compose readiness before reporting success. Edge readiness does
not assert backend database readiness.

Run the native regression with Docker and host OpenSSL:

```sh
python3 -m unittest tests.compose.test_install_https -v
```

The tests use stock `nginx:1.30-alpine` (or the configured nginx image), a
temporary loopback-published network, a 128 MiB container limit and the actual
Compose health command. It builds no product images and uses no credentials
from an existing installation.
