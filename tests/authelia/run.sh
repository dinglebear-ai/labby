#!/usr/bin/env bash
set -euo pipefail

readonly image="authelia/authelia:4.39.10"
fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp_dir="$(mktemp -d)"
readonly name="labby-authelia-acceptance-$(basename "${tmp_dir}")"
readonly port="$(ruby -rsocket -e 's=TCPServer.new("127.0.0.1",0); puts s.addr[1]; s.close')"

cleanup() {
  docker rm -f "${name}" >/dev/null 2>&1 || true
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT INT TERM

openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=Labby Acceptance CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "${tmp_dir}/ca.key" -out "${tmp_dir}/ca.crt" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj "/CN=auth.localhost" \
  -addext "subjectAltName=DNS:auth.localhost" \
  -keyout "${tmp_dir}/tls.key" -out "${tmp_dir}/tls.csr" >/dev/null 2>&1
printf 'subjectAltName=DNS:auth.localhost\nextendedKeyUsage=serverAuth\n' >"${tmp_dir}/tls.ext"
openssl x509 -req -days 1 -in "${tmp_dir}/tls.csr" -CA "${tmp_dir}/ca.crt" \
  -CAkey "${tmp_dir}/ca.key" -CAcreateserial -extfile "${tmp_dir}/tls.ext" \
  -out "${tmp_dir}/tls.crt" >/dev/null 2>&1
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
  -out "${tmp_dir}/oidc.key" >/dev/null 2>&1
cp "${fixture_dir}/users_database.yml" "${tmp_dir}/users_database.yml"

cat >"${tmp_dir}/configuration.yml" <<'YAML'
server:
  address: 'tcp://0.0.0.0:9091'
  tls:
    key: '/config/tls.key'
    certificate: '/config/tls.crt'
log:
  level: 'info'
identity_validation:
  reset_password:
    jwt_secret: 'acceptance-reset-secret-at-least-thirty-two-bytes'
authentication_backend:
  file:
    path: '/config/users_database.yml'
access_control:
  default_policy: 'one_factor'
session:
  secret: 'acceptance-session-secret-at-least-thirty-two-bytes'
  cookies:
    - name: 'authelia_session'
      domain: 'auth.localhost'
      authelia_url: 'https://auth.localhost:__PORT__'
      default_redirection_url: 'https://app.auth.localhost'
storage:
  encryption_key: 'acceptance-storage-key-at-least-thirty-two-bytes'
  local:
    path: '/config/db.sqlite3'
notifier:
  filesystem:
    filename: '/config/notification.txt'
identity_providers:
  oidc:
    hmac_secret: 'acceptance-oidc-hmac-secret-at-least-sixty-four-bytes-000000000000'
    jwks:
      - key_id: 'labby-test'
        algorithm: 'RS256'
        use: 'sig'
        key: |
__OIDC_KEY__
    claims_policies:
      labby:
        id_token: ['email', 'email_verified', 'preferred_username', 'name']
    clients:
      - client_id: 'labby-acceptance'
        client_name: 'Labby Acceptance'
        client_secret: '$pbkdf2-sha512$310000$kFEg7wwGGllF5QcYOTiFIw$REj7Gkabe/N6LwRdyPnSUxr5tTflP6dRaJJ/IedkW4MhtP3exNtfMqwz8ynGE4qslZbJBCGAMcNXWAihtTQwZg'
        public: false
        authorization_policy: 'one_factor'
        claims_policy: 'labby'
        consent_mode: 'implicit'
        require_pkce: true
        pkce_challenge_method: 'S256'
        redirect_uris:
          - 'https://labby.localhost/auth/oidc/callback'
        scopes: ['openid', 'profile', 'email']
        response_types: ['code']
        grant_types: ['authorization_code']
        token_endpoint_auth_method: 'client_secret_basic'
YAML
sed -i.bak "s/__PORT__/${port}/g" "${tmp_dir}/configuration.yml"
rm -f "${tmp_dir}/configuration.yml.bak"
awk -v key_file="${tmp_dir}/oidc.key" '
  /^__OIDC_KEY__$/ {
    while ((getline line < key_file) > 0) print "          " line
    close(key_file)
    next
  }
  { print }
' "${tmp_dir}/configuration.yml" >"${tmp_dir}/configuration.rendered.yml"
mv "${tmp_dir}/configuration.rendered.yml" "${tmp_dir}/configuration.yml"

docker run -d --name "${name}" --user "$(id -u):$(id -g)" \
  -p "127.0.0.1:${port}:9091" \
  -v "${tmp_dir}:/config" "${image}" >/dev/null

ready=0
for _ in $(seq 1 60); do
  if curl --silent --fail --cacert "${tmp_dir}/ca.crt" \
    --resolve "auth.localhost:${port}:127.0.0.1" \
    "https://auth.localhost:${port}/api/health" >/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "${ready}" != 1 ]]; then
  docker logs --tail 200 "${name}" 2>&1 | sed -E 's/(secret|password|token)=[^ ]+/\1=<redacted>/gi' >&2
  exit 1
fi

LABBY_AUTHELIA_ACCEPTANCE=1 \
LABBY_AUTHELIA_ACCEPTANCE_ISSUER="https://auth.localhost:${port}" \
LABBY_AUTHELIA_ACCEPTANCE_CA="${tmp_dir}/ca.crt" \
cargo test -p labby-auth --all-features --test authelia_acceptance \
  -- --ignored --exact real_authelia_authorization_code_pkce_flow
