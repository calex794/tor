#!/usr/bin/env bash
# DEPLOY-08a: only the disposable dedicated GitHub-hosted Ubuntu VM.
# No production SSH, deployment environment, private key or user data.
set -Eeuo pipefail
umask 077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LANG=C.UTF-8 LC_ALL=C.UTF-8
[[ $EUID == 0 && ${GITHUB_ACTIONS:-} == true &&
   ${RUNNER_ENVIRONMENT:-} == github-hosted && ${RUNNER_OS:-} == Linux &&
   $(cat /proc/1/comm) == systemd && ${GITHUB_SHA:-} =~ ^[0-9a-f]{40}$ &&
   ${GITHUB_WORKSPACE:-} == "$PWD" ]] || {
  echo 'REFUSE: disposable GitHub-hosted VM only' >&2; exit 2;
}
case "${GITHUB_EVENT_NAME:-}" in pull_request|workflow_dispatch) ;; *) exit 2;; esac
. /etc/os-release
[[ $ID == ubuntu && $VERSION_ID == 24.04 ]] || exit 2
for path in /opt/tor /etc/tor /var/lib/tor-release /var/lib/tor-edge \
  /srv/tor-install /srv/tor-recovery /usr/local/lib/tor-release-v2 \
  /usr/local/sbin/tor-one-click-release /etc/systemd/system/tor-web.service \
  /etc/systemd/system/tor-web.service.d /etc/systemd/system/tor-trace-store.service \
  /etc/sudoers.d/tor-one-click-release /root/tor-deploy-lab \
  /etc/letsencrypt/live/pandaolate.ru; do
  [[ ! -e "$path" && ! -L "$path" ]] || { echo 'REFUSE: existing TOR/fixture path' >&2; exit 2; }
done
if getent passwd tor-deploy >/dev/null; then exit 2; fi
install -d -m 0700 /root/tor-deploy-lab
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl gpg postgresql-common \
  nginx openssl openssh-client iproute2
# Same authenticated PGDG distribution as the operator bootstrap.
key=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
install -d -m 0755 "$(dirname "$key")"
curl --disable --fail --silent --show-error --location --retry 3 --max-time 30 \
  https://www.postgresql.org/media/keys/ACCC4CF8.asc -o "$key"
fingerprint=$(gpg --batch --show-keys --with-colons "$key" | awk -F: '$1=="fpr"{print $10;exit}')
[[ "$fingerprint" == B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8 ]] || {
  echo 'REFUSE: unexpected PGDG public key fingerprint' >&2; exit 2;
}
chmod 0644 "$key"
cat > /etc/apt/sources.list.d/tor-deploy-lab-pgdg.sources <<EOF
Types: deb
URIs: https://apt.postgresql.org/pub/repos/apt
Suites: noble-pgdg
Architectures: $(dpkg --print-architecture)
Components: main
Signed-By: $key
EOF
chmod 0644 /etc/apt/sources.list.d/tor-deploy-lab-pgdg.sources
apt-get update -qq
apt-get install -y -qq --no-install-recommends postgresql-18
# Hosted images may include a default package cluster. Do not delete any
# preexisting PGDATA. Stop only recognized default clusters and preserve
# their configs in the private disposable fixture directory.
while read -r version cluster port status rest; do
  [[ "$version" =~ ^[0-9]+$ && "$cluster" == main && "$port" =~ ^[0-9]+$ ]] || exit 2
  case "$status" in
    online) systemctl stop "postgresql@$version-$cluster.service";;
    down) ;;
    *) echo 'REFUSE: unexpected runner cluster state' >&2; exit 2;;
  esac
done < <(pg_lsclusters -h)
if [[ -d /etc/postgresql ]]; then
  mv -T /etc/postgresql /root/tor-deploy-lab/unused-runner-pg-config
fi
install -d -m 0755 /etc/postgresql
[[ ! -e /var/lib/postgresql/18/tor-deploy-lab ]] || exit 2
pg_createcluster --start-conf=manual --datadir /var/lib/postgresql/18/tor-deploy-lab \
  18 main -- --auth-local=peer --auth-host=scram-sha-256
pg_conftool 18 main set port 5432
pg_conftool 18 main set listen_addresses 127.0.0.1
pg_conftool 18 main set password_encryption scram-sha-256
pg_conftool 18 main set max_connections 20
pg_conftool 18 main set shared_buffers 64MB
systemctl daemon-reload
systemctl start postgresql@18-main.service
runuser -u postgres -- psql -X -Atqc 'SHOW server_version_num' | grep -Eq '^18[0-9]{4}$'
# Remove only the standard package default link, not arbitrary vhosts.
default=/etc/nginx/sites-enabled/default
if [[ -e "$default" || -L "$default" ]]; then
  [[ -L "$default" && $(readlink -f "$default") == /etc/nginx/sites-available/default ]] || exit 2
  unlink "$default"
fi
[[ -z $(find /etc/nginx/sites-enabled /etc/nginx/conf.d -mindepth 1 -maxdepth 1 -print -quit) ]] || exit 2
# Two workers are sufficient for generation fencing, avoid dozens on CI VMs.
sed -i -E 's/^worker_processes[[:space:]]+auto;/worker_processes 2;/' /etc/nginx/nginx.conf
nginx -t
systemctl restart nginx.service
useradd --create-home --user-group --shell /usr/sbin/nologin tor-deploy
install -d -m 0700 /etc/letsencrypt/live/pandaolate.ru
openssl req -x509 -newkey rsa:2048 -nodes -days 3 \
  -subj '/CN=pandaolate.ru' -addext 'subjectAltName=DNS:pandaolate.ru' \
  -keyout /etc/letsencrypt/live/pandaolate.ru/privkey.pem \
  -out /etc/letsencrypt/live/pandaolate.ru/fullchain.pem >/dev/null 2>&1
chmod 0600 /etc/letsencrypt/live/pandaolate.ru/privkey.pem
install -m 0644 /etc/letsencrypt/live/pandaolate.ru/fullchain.pem \
  /usr/local/share/ca-certificates/tor-deploy-lab.crt
update-ca-certificates >/dev/null
# Real localhost TLS verification uses normal trust, never curl -k.
printf '%s\n' "$GITHUB_SHA" > /root/tor-deploy-lab/approved-checkout
echo 'DEPLOY08A_HOSTED_VM_FIXTURE_READY'
