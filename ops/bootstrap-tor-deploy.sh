#!/usr/bin/env bash
# One-time TOR GitHub Actions SSH key setup on VPS. Does not touch Nginx.
set -euo pipefail
umask 077

if [[ "$(id -u)" != "0" ]]; then
  echo "Run as root" >&2
  exit 1
fi

user="tor-deploy"
id "$user" >/dev/null 2>&1 || useradd -m -s /bin/bash "$user"
home="$(getent passwd "$user" | cut -d: -f6)"
if [[ "$home" != "/home/tor-deploy" ]]; then
  echo "Unexpected tor-deploy home; refusing to change SSH access" >&2
  exit 1
fi
install -d -o "$user" -g "$user" -m 0750 /opt/tor /opt/tor/releases
install -d -o "$user" -g "$user" -m 0700 "$home/.ssh"

key_dir="/root/.config/tor"
private="$key_dir/tor_deploy_ed25519"
public="$private.pub"
authorized="$home/.ssh/authorized_keys"
install -d -m 0700 "$key_dir"
if [[ ! -e "$private" && ! -e "$public" ]]; then
  ssh-keygen -q -t ed25519 -N "" -f "$private" -C "tor-github-actions"
elif [[ ! -s "$private" || ! -s "$public" ]]; then
  echo "Partial or already decommissioned private key: refusing silent rotation" >&2
  exit 1
fi
chmod 0600 "$private"
chmod 0644 "$public"
if [[ ! -e "$authorized" ]]; then
  install -o "$user" -g "$user" -m 0600 /dev/null "$authorized"
fi
pub_line="$(cat "$public")"
if [[ "$pub_line" != ssh-ed25519\ * ]]; then
  echo "Unexpected public key format" >&2
  exit 1
fi
# Restrict port/agent forwarding, X11 and PTY; SSH exec and SCP remain allowed.
grep -Fq -- "$pub_line" "$authorized" || printf 'restrict %s\n' "$pub_line" >> "$authorized"
chown "$user:$user" "$authorized"
chmod 0600 "$authorized"

host_pub="/etc/ssh/ssh_host_ed25519_key.pub"
if [[ ! -r "$host_pub" ]]; then echo "Missing SSH host ED25519 public key" >&2; exit 1; fi
read -r algo host_key _ < "$host_pub"
if [[ "$algo" != "ssh-ed25519" || -z "$host_key" ]]; then echo "Invalid SSH host key" >&2; exit 1; fi
host="$(printenv TOR_DEPLOY_HOST || true)"
if [[ -z "$host" ]] && command -v curl >/dev/null 2>&1; then
  host="$(curl -4fsS --max-time 7 https://api.ipify.org || true)"
fi
echo "Deploy public key fingerprint:"
ssh-keygen -lf "$public"
echo "Server SSH host fingerprint (verify against VPS console/provider):"
ssh-keygen -lf "$host_pub"
if [[ -n "$host" && "$host" =~ ^[A-Za-z0-9.-]+$ ]]; then
  printf '%s %s %s\n' "$host" "$algo" "$host_key" > "$key_dir/known_hosts"
  chmod 0600 "$key_dir/known_hosts"
  printf '\nTOR_DEPLOY_HOST: %s (verify in provider panel)\n' "$host"
  echo "Known-hosts public line saved: $key_dir/known_hosts"
else
  echo "No public address detected. Rerun as TOR_DEPLOY_HOST=VERIFIED_IP bash SCRIPT"
fi

cat <<EOF

GitHub calex794/tor -> Settings -> Environments -> tor-staging:
  Environment Secret TOR_DEPLOY_SSH_KEY: contents of $private (PRIVATE: copy ONLY into GitHub, NEVER CHAT).
  Environment Secret TOR_DEPLOY_KNOWN_HOSTS: contents of $key_dir/known_hosts.
  Environment Variable TOR_DEPLOY_HOST: verified IP/hostname.
  Environment Variable TOR_DEPLOY_USER: tor-deploy.
GitHub repository Actions variable TOR_AUTO_DEPLOY=false.

After the first successful stage-only SSH test, remove temporary VPS private key:
  rm -f $private

No running server or Nginx configuration was changed.
EOF
