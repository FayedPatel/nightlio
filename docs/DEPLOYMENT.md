# 🚀 Production Deployment Guide

This guide covers deploying Nightlio in production environments with proper security, performance, and reliability.

Nightlio ships a single `docker-compose.yml` for every environment — local,
LAN, and public production. There is no separate prod compose file; for
public deployments you put your own TLS-terminating reverse proxy (Caddy,
Traefik, or nginx) in front of the `frontend` service instead.

## Quick Production Setup

```bash
# Clone and configure
git clone https://github.com/FayedPatel/nightlio.git
cd nightlio
cp .env.docker .env

# Generate secure secrets and write them into .env (both start empty and
# are required — compose refuses to start the api without them)
sed -i "s/^SECRET_KEY=.*/SECRET_KEY=$(openssl rand -hex 32)/" .env
sed -i "s/^JWT_SECRET=.*/JWT_SECRET=$(openssl rand -hex 32)/" .env

# Deploy (builds from source; drop --build to use API_IMAGE/WEB_IMAGE instead)
docker compose up -d --build
```

For a public deployment, also set in `.env`:

```bash
TRUST_PROXY_HEADERS=1
CORS_ORIGINS=https://yourdomain.com
```

then put a reverse proxy in front of the `frontend` container's port
(`5173` by default) to terminate TLS — see "Reverse Proxy Setup" below.

## Environment Configuration

### Required Production Variables

Create a `.env` file with:

```bash
# Security (REQUIRED - Generate random values!)
SECRET_KEY=your-64-character-random-secret-here
JWT_SECRET=your-different-64-character-secret-here

# Domain configuration
CORS_ORIGINS=https://yourdomain.com,https://www.yourdomain.com

# Environment selector (D5: APP_ENV replaces RAILWAY_ENVIRONMENT; the old
# name is still honored as a fallback if already set)
APP_ENV=production

# Optional features
ENABLE_MOOD_MUSIC=0
# Web3 removed
DEFAULT_SELF_HOST_ID=selfhost_default_user

# OIDC single sign-on (optional; any OIDC-compliant provider, Pocket ID
# recommended -- see docs/DOCKER.md for the bundled compose profile).
# Leave empty for local-password/single-user auth.
OIDC_ISSUER_URL=
OIDC_CLIENT_ID=
OIDC_CLIENT_SECRET=

# Mood music (if enabled)
JAMENDO_CLIENT_ID=your-jamendo-client-id
```

### Generating Secure Secrets

```bash
# Generate SECRET_KEY
openssl rand -hex 32

# Generate JWT_SECRET (use different value)
openssl rand -hex 32
```

## Server Requirements

### Minimum Requirements
- **RAM**: 512 MB
- **CPU**: 1 vCPU
- **Storage**: 1 GB (grows with data)
- **OS**: Any Linux distribution with Docker support

### Recommended Requirements
- **RAM**: 1 GB
- **CPU**: 2 vCPU
- **Storage**: 5 GB SSD
- **OS**: Ubuntu 22.04 LTS

## SSL/HTTPS Setup

Nightlio's `frontend` container serves plain HTTP only — it does not bundle
a TLS-terminating nginx config. Run an external reverse proxy in front of it
and let that proxy handle certificates.

### Option 1: Let's Encrypt with Certbot (behind your own nginx)

```bash
# Install certbot
sudo apt update
sudo apt install certbot python3-certbot-nginx

# Get SSL certificate
sudo certbot --nginx -d yourdomain.com

# Auto-renewal (add to crontab)
echo "0 12 * * * /usr/bin/certbot renew --quiet" | sudo crontab -
```

Point that external nginx's `proxy_pass` at the `frontend` container's
published port (`http://localhost:5173` by default) — see "Reverse Proxy
Setup" below for the site config.

### Option 2: Caddy (automatic HTTPS, no separate certbot step)

Caddy obtains and renews Let's Encrypt certificates on its own — no `ssl/`
directory or manual cert placement needed. See the Caddy example under
"Reverse Proxy Setup" below.

## Reverse Proxy Setup

### Nginx (External)

```nginx
server {
    listen 80;
    server_name yourdomain.com;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name yourdomain.com;

    ssl_certificate /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://localhost:5173;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### Traefik

```yaml
services:
  nightlio-api:
    # ... your api config
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.nightlio-api.rule=Host(`yourdomain.com`) && PathPrefix(`/api`)"
      - "traefik.http.routers.nightlio-api.tls.certresolver=letsencrypt"

  nightlio-frontend:
    # ... your frontend config
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.nightlio.rule=Host(`yourdomain.com`)"
      - "traefik.http.routers.nightlio.tls.certresolver=letsencrypt"
```

### Caddy

```
yourdomain.com {
    reverse_proxy localhost:5173
}
```

## Monitoring & Maintenance

### Health Checks

```bash
# Check service status
docker compose ps

# View logs
docker compose logs -f

# Check resource usage
docker stats
```

### Backup Strategy

```bash
#!/bin/bash
# backup.sh - Run daily via cron

BACKUP_DIR="/backups/nightlio"
DATE=$(date +%Y%m%d_%H%M%S)

# Create backup directory
mkdir -p $BACKUP_DIR

# Backup database
docker run --rm \
  -v nightlio_nightlio_data:/data \
  -v $BACKUP_DIR:/backup \
  alpine tar czf /backup/nightlio-$DATE.tar.gz -C /data .

# Keep only last 7 days
find $BACKUP_DIR -name "nightlio-*.tar.gz" -mtime +7 -delete

# Upload to cloud storage (optional)
# aws s3 cp $BACKUP_DIR/nightlio-$DATE.tar.gz s3://your-bucket/
```

### Log Rotation

Add to `/etc/logrotate.d/docker-nightlio`:

```
/var/lib/docker/containers/*/*-json.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0644 root root
    postrotate
        docker kill -s USR1 $(docker ps -q) 2>/dev/null || true
    endscript
}
```

## Security Hardening

### Docker Security

All images already run as non-root users out of the box: the api as
`appuser` (uid 1000, see `api/Dockerfile`), the frontend on the
`nginxinc/nginx-unprivileged` base (uid 101, listening on 8080), and the
No Dockerfile edits needed.

Optional extra hardening in `docker-compose.yml`:

```yaml
security_opt:
  - no-new-privileges:true
```

### Firewall Configuration

```bash
# UFW (Ubuntu)
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow ssh
sudo ufw allow 80
sudo ufw allow 443
sudo ufw enable

# iptables
iptables -A INPUT -p tcp --dport 22 -j ACCEPT
iptables -A INPUT -p tcp --dport 80 -j ACCEPT
iptables -A INPUT -p tcp --dport 443 -j ACCEPT
iptables -A INPUT -j DROP
```

### Regular Updates

```bash
#!/bin/bash
# update.sh - Run weekly

# Update system
sudo apt update && sudo apt upgrade -y

# Update Docker images
cd /path/to/nightlio
git pull
docker compose down
docker compose build --no-cache
docker compose up -d

# Cleanup old images
docker image prune -f
```

For version-specific upgrade notes — including the v0.4.0 cutover that
replaced the Python API with the Rust binary on the same data volume — see
[UPGRADING.md](UPGRADING.md).

## Performance Optimization

### Database Optimization

The database deliberately stays in SQLite's default (rollback-journal) mode
— do **not** enable WAL. Switching to WAL leaves `-wal`/`-shm` companion
files next to the database that older API images cannot safely coexist
with, which would break rolling back to a pre-v0.4.0 (Flask) image.
Enabling WAL is deferred until the legacy rollback path is permanently
retired (a separate owner decision — see the "Legacy removal" entry in
`contract/DECISIONS.md`). The api image intentionally ships no Python or
sqlite3 CLI, so there is no supported in-container way to flip pragmas by
hand anyway.

### Nginx Caching

Add to nginx configuration:

```nginx
# Cache static files
location ~* \.(js|css|png|jpg|jpeg|gif|ico|svg)$ {
    expires 1y;
    add_header Cache-Control "public, immutable";
}

# Enable gzip
gzip on;
gzip_types text/plain text/css application/json application/javascript text/xml application/xml;
```

## Troubleshooting

### Common Issues

1. **Services won't start**
   ```bash
   docker compose logs
   # Check port conflicts
   netstat -tlnp | grep -E ':(80|443|5173|5000)'
   ```

2. **Database corruption**
   ```bash
   # Restore from backup
   docker compose down
   docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup alpine tar xzf /backup/nightlio-backup.tar.gz -C /data
   docker compose up -d
   ```

3. **High memory usage**
   ```bash
   # Add memory limits to docker-compose.yml
   deploy:
     resources:
       limits:
         memory: 512M
   ```

### Getting Help

1. Check logs: `docker compose logs -f`
2. Verify configuration: `docker compose config`
3. Test connectivity: `curl -f http://localhost:5173`
4. Create GitHub issue with logs and configuration

## Scaling

### Horizontal Scaling

```yaml
# docker-compose.yml
services:
  api:
    deploy:
      replicas: 3
    # Add load balancer
  
  nginx:
    # Configure upstream servers
```

### Database Scaling

For high-traffic deployments, consider:
- PostgreSQL instead of SQLite
- Read replicas
- Connection pooling
- Database clustering

## Cloud Deployment

### AWS ECS

```bash
# Build and push to ECR
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin 123456789012.dkr.ecr.us-east-1.amazonaws.com
docker build -t nightlio .
docker tag nightlio:latest 123456789012.dkr.ecr.us-east-1.amazonaws.com/nightlio:latest
docker push 123456789012.dkr.ecr.us-east-1.amazonaws.com/nightlio:latest
```

### Google Cloud Run

```bash
# Build and deploy
gcloud builds submit --tag gcr.io/PROJECT-ID/nightlio
gcloud run deploy --image gcr.io/PROJECT-ID/nightlio --platform managed
```

### DigitalOcean App Platform

```yaml
# .do/app.yaml
name: nightlio
services:
- name: api
  source_dir: /api
  dockerfile_path: Dockerfile
  http_port: 5000
- name: web
  source_dir: /
  dockerfile_path: Dockerfile
  http_port: 80
```

---

**Need help?** Open an issue on GitHub or check the main [Docker guide](DOCKER.md) for basic setup.
