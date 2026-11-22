# Deployment files

- `systemd/netwatch-poller.service` and `systemd/netwatch-api.service`: service units. Install to `/etc/systemd/system/`.
- `netwatch.env.example`: environment file template. Install to `/etc/netwatch/netwatch.env`.
- `nginx/netwatch.conf`: HTTPS reverse proxy example.

The full procedure, including the database, the encryption key, and migrations, is in `../DEPLOY.md`.
These files have not been tested on a server. Test on a staging host first.
