# Deploying NetWatch

This guide installs NetWatch on a single Linux server with PostgreSQL, two systemd services, and an HTTPS reverse proxy. Commands assume Debian or Ubuntu. Adjust package names for other distributions.

> **Before you start:** NetWatch is not yet production-validated. Read the [Known limitations](README.md#known-limitations) in the README, and tell your network team before enabling scanning.

## 1. What you will run

| Component | Binary | Runs as | Listens on |
|---|---|---|---|
| Poller | `netwatch-poller` | `netwatch` service user | nothing (outbound SNMP and ICMP only) |
| API and dashboard | `netwatch-api` | `netwatch` service user | `127.0.0.1:8080` |
| Admin tools | `netwatch-migrate`, `netwatch-user` | run by hand | nothing |
| Database | PostgreSQL | `postgres` | `127.0.0.1:5432` |
| HTTPS proxy | nginx (example) | `www-data` | `443` |

The poller needs outbound access to devices on UDP 161 (SNMP) and ICMP. The API needs no inbound access except through the proxy.

## 2. Prerequisites

Install PostgreSQL, nginx, and build tools on the server:

```sh
sudo apt update
sudo apt install -y postgresql nginx build-essential pkg-config libssl-dev openssl curl git
```

Install Rust with rustup on the build machine. The build machine can be the server itself or a separate host with the same operating system:

```sh
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
```

Install Node.js 20.19 or newer for the frontend build. Use your usual method, for example `nvm`, or the NodeSource repository.

## 3. Create the service user and directories

```sh
sudo useradd --system --home /opt/netwatch --shell /usr/sbin/nologin netwatch
sudo install -d -o root -g root -m 0755 /opt/netwatch /opt/netwatch/bin /opt/netwatch/web
sudo install -d -o root -g netwatch -m 0750 /etc/netwatch
```

## 4. Create the database

Choose a strong password and keep it. The example uses `change-me`.

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE netwatch LOGIN PASSWORD 'change-me';
CREATE DATABASE netwatch OWNER netwatch;
SQL
```

The `netwatch` role must own the database, because migrations create tables, partitions, and functions. Do not grant it superuser rights.

Confirm the connection works:

```sh
psql "postgres://netwatch:change-me@127.0.0.1/netwatch" -c 'SELECT version();'
```

Keep PostgreSQL listening on `127.0.0.1` unless the poller runs on another host. If it does, use TLS for the connection and restrict access with `pg_hba.conf`.

## 5. Create the encryption key

The poller and API share one key. It encrypts stored SNMP credentials. Losing it means every device credential must be entered again, so back it up separately from the database.

```sh
openssl rand -hex 32 | sudo tee /etc/netwatch/secret.key > /dev/null
sudo chown netwatch:netwatch /etc/netwatch/secret.key
sudo chmod 0600 /etc/netwatch/secret.key
```

The file must contain exactly 64 hexadecimal characters on one line. Check it without printing the key:

```sh
sudo -u netwatch wc -c /etc/netwatch/secret.key   # expect 65 (64 characters plus newline)
```

Do not use `NETWATCH_KEY_HEX` in production. Environment variables can be read by other processes on the host.

## 6. Build the release binaries

On the build machine, from the repository root:

```sh
cargo build --release -p netwatch-poller -p netwatch-api --bins
npm --prefix web ci
npm --prefix web run build
```

This produces these files in `target/release/`: `netwatch-poller`, `netwatch-api`, `netwatch-migrate`, and `netwatch-user`. The migration files are compiled into the binaries, so the server does not need the source tree to migrate.

Copy the files to the server. Replace `user@server` with your login:

```sh
scp target/release/netwatch-poller target/release/netwatch-api \
    target/release/netwatch-migrate target/release/netwatch-user user@server:/tmp/
scp -r web/dist user@server:/tmp/netwatch-web-dist
```

On the server:

```sh
sudo install -o root -g root -m 0755 /tmp/netwatch-poller /tmp/netwatch-api \
    /tmp/netwatch-migrate /tmp/netwatch-user /opt/netwatch/bin/
sudo rm -rf /opt/netwatch/web/dist
sudo mv /tmp/netwatch-web-dist /opt/netwatch/web/dist
sudo chown -R root:root /opt/netwatch/web/dist
```

## 7. Write the environment file

Create `/etc/netwatch/netwatch.env`. Its contents include the database password, so restrict who can read it:

```sh
sudo tee /etc/netwatch/netwatch.env > /dev/null <<'EOF'
DATABASE_URL=postgres://netwatch:change-me@127.0.0.1/netwatch
NETWATCH_KEY_FILE=/etc/netwatch/secret.key
# Ranges that scans may target. Leave empty to disable scanning.
NETWATCH_SCAN_ALLOW=10.0.0.0/8
NETWATCH_API_BIND=127.0.0.1:8080
NETWATCH_WEB_ROOT=/opt/netwatch/web/dist
RUST_LOG=info
EOF
sudo chown root:netwatch /etc/netwatch/netwatch.env
sudo chmod 0640 /etc/netwatch/netwatch.env
```

Set `NETWATCH_SCAN_ALLOW` to the ranges you are permitted to scan, for example `10.0.0.0/8,192.168.1.0/24`. Do not widen it without approval.

## 8. Apply migrations and create an admin account

Run the migration tool once. It applies every migration in order and does nothing if the database is current:

```sh
sudo bash -c 'set -a; . /etc/netwatch/netwatch.env; set +a; /opt/netwatch/bin/netwatch-migrate'
```

The poller also runs migrations when it starts, so this step is a safe first run. It separates migration problems from runtime problems.

Create the first admin account. The password is read from standard input, not from command-line arguments:

```sh
read -r -s -p 'Admin password: ' NW_PASS; echo
printf '%s\n' "$NW_PASS" | sudo bash -c 'set -a; . /etc/netwatch/netwatch.env; set +a; /opt/netwatch/bin/netwatch-user admin admin'
unset NW_PASS
```

The arguments are the username and the role (`admin` or `viewer`). Passwords must be between 12 and 1024 bytes. Anyone with database write access can create accounts, so run this only on the server.

## 9. Install the systemd services

Create the poller unit at `/etc/systemd/system/netwatch-poller.service`:

```ini
[Unit]
Description=NetWatch poller
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=netwatch
Group=netwatch
EnvironmentFile=/etc/netwatch/netwatch.env
ExecStart=/opt/netwatch/bin/netwatch-poller
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

Create the API unit at `/etc/systemd/system/netwatch-api.service`:

```ini
[Unit]
Description=NetWatch API and dashboard
After=network-online.target postgresql.service netwatch-poller.service
Wants=network-online.target

[Service]
Type=simple
User=netwatch
Group=netwatch
EnvironmentFile=/etc/netwatch/netwatch.env
ExecStart=/opt/netwatch/bin/netwatch-api
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

Enable and start both:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now netwatch-poller netwatch-api
```

The `TimeoutStopSec=30` value gives the poller time to flush its buffers on shutdown. These units are examples. Test them on a staging host first, and adjust sandboxing options if the services fail to start.

## 10. Put HTTPS in front of the API

The API serves plain HTTP. Passwords and session tokens must not cross the network unencrypted, so terminate TLS at nginx. Use a certificate from your internal CA.

Create `/etc/nginx/sites-available/netwatch`:

```nginx
server {
    listen 443 ssl;
    server_name netwatch.example.internal;

    ssl_certificate     /etc/ssl/netwatch/netwatch.crt;
    ssl_certificate_key /etc/ssl/netwatch/netwatch.key;
    ssl_protocols       TLSv1.2 TLSv1.3;

    client_max_body_size 64k;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_read_timeout 60s;
    }
}

server {
    listen 80;
    server_name netwatch.example.internal;
    return 301 https://$host$request_uri;
}
```

Enable it and reload:

```sh
sudo ln -s /etc/nginx/sites-available/netwatch /etc/nginx/sites-enabled/netwatch
sudo nginx -t && sudo systemctl reload nginx
```

The API does not read forwarded IP headers. Its login rate limit uses the address nginx presents, so every user behind the proxy shares one limit. Add rate limiting in nginx if you have many users.

Restrict the certificate's private key to root:

```sh
sudo chmod 0600 /etc/ssl/netwatch/netwatch.key
```

## 11. Verify the installation

Check both services:

```sh
systemctl status netwatch-poller netwatch-api --no-pager
curl -s http://127.0.0.1:8080/healthz; echo
curl -sk https://netwatch.example.internal/healthz; echo
```

Watch the poller logs:

```sh
journalctl -u netwatch-poller -f
```

Sign in at `https://netwatch.example.internal/` with the admin account. Add one device through the scanner or the API, then confirm it appears on the dashboard and its status updates within a few minutes.

If the status stays stale, check that the device answers SNMP from the server:

```sh
snmpget -v2c -c <community> <device-ip> 1.3.6.1.2.1.1.1.0
```

Use `snmpget` from the `snmp` package. For SNMP v3, use the matching `-v3` options.

## 12. Backups

Back up the database every night, and keep the key file in a separate, secured location:

```sh
sudo -u postgres pg_dump -Fc netwatch | gzip > /var/backups/netwatch-$(date +%F).dump.gz
sudo cp -p /etc/netwatch/secret.key /path/to/secure/offline/location/
```

Test a restore on a different host before you rely on the backups. Restoring the database without the matching key leaves stored device credentials unreadable.

Retention settings control how much raw data is kept. Partitions are dropped by the poller, so the database does not need manual cleanup.

## 13. Upgrading

1. Take a database backup (section 12).
2. Build the new binaries and frontend on the build machine (section 6).
3. Stop the services: `sudo systemctl stop netwatch-api netwatch-poller`.
4. Install the new binaries and replace `web/dist`.
5. Run `netwatch-migrate` (section 8).
6. Start the services and check the logs.

Do not change the key file during an upgrade unless you have a re-encryption procedure. The runbook describes key rotation.

## 14. Troubleshooting

| Symptom | Likely cause | What to check |
|---|---|---|
| Poller exits at start with a key error | Key file missing, wrong permissions, or not 64 hex characters | Section 5; `journalctl -u netwatch-poller` |
| `DATABASE_URL is required` | Environment file not loaded | `EnvironmentFile=` in the unit; file permissions |
| Login fails for an account you created | Wrong role or password | Recreate with `netwatch-user` |
| Scans find nothing | Wrong credential profile, or device blocks SNMP | Test with `snmpget` from the server; check the profile |
| Scan refuses a range | Range outside `NETWATCH_SCAN_ALLOW` or larger than /22 | The environment file |
| Dashboard shows devices as stale | No recent poll result | Poller logs; SNMP reachability from the server |
| Browser shows a blank page | `web/dist` missing or `NETWATCH_WEB_ROOT` wrong | `ls /opt/netwatch/web/dist` |
| nginx returns 502 | API not running on `127.0.0.1:8080` | `systemctl status netwatch-api` |

## 15. Uninstalling

```sh
sudo systemctl disable --now netwatch-api netwatch-poller
sudo rm /etc/systemd/system/netwatch-api.service /etc/systemd/system/netwatch-poller.service
sudo systemctl daemon-reload
sudo rm -rf /opt/netwatch /etc/netwatch
sudo -u postgres dropdb netwatch
sudo -u postgres dropuser netwatch
```

Take a final backup first. Dropping the database is permanent.
