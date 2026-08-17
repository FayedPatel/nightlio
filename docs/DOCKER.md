# 🐳 Docker Quick Start Guide for Nightlio

This guide will help you get Nightlio running with Docker in just a few minutes.

## Prerequisites

- Docker and Docker Compose installed on your system
- Basic familiarity with command line

## Quick Start

1. **Clone the repository**
   ```bash
   git clone https://github.com/FayedPatel/nightlio.git
   cd nightlio
   ```

2. **Create environment file**
   ```bash
   cp .env.docker .env
   ```

3. **Edit the environment file** (Important for security!)
   ```bash
   nano .env  # or use your preferred editor
   ```
   
   **⚠️ IMPORTANT**: Change at least these values:
   - `SECRET_KEY`: Use a long, random string
   - `JWT_SECRET`: Use a different long, random string

4. **Start the application**
   ```bash
   docker compose up -d --build
   ```

5. **Access your application**
   - Open your browser and go to `http://localhost:5173`
   - The API will be available at `http://localhost:5000`

## Port Information

- **Frontend**: `http://localhost:5173` - Same port for both Docker and development (consistency!)
- **API**: `http://localhost:5000` - Rust backend (same in both Docker and development)

## Environment Configuration

### Required Settings
- `SECRET_KEY`: API secret key, signs the OIDC state cookie (change this!)
- `JWT_SECRET`: JWT signing secret (change this!)

### Optional Features
- `OIDC_ISSUER_URL` / `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET`: set all three to enable OIDC single sign-on (leave empty for local-password/single-user auth)
- `DISABLE_LOCAL_LOGIN`: set to `1` on SSO-only deployments to hard-disable `POST /api/auth/local/login` entirely (password form and credential-free mode) — the identity provider becomes the only way in. Warning: with OIDC unconfigured AND this set, nobody can log in.
- `ENABLE_MOOD_MUSIC`: Set to `1` to enable mood-based music recommendations
- `DEFAULT_SELF_HOST_ID`: User ID for self-hosted instances
- `APP_ENV`: environment selector (`production`/`development`); replaces `RAILWAY_ENVIRONMENT`, which is still honored as a fallback
- `FRONTEND_URL`: optional, only needed if the frontend is served from a different origin than the api
- `TRUST_PROXY_HEADERS`: optional, set to `1` only if the api sits behind a trusted reverse proxy

### OIDC Setup (Optional)

Nightlio speaks generic OpenID Connect -- any spec-compliant provider works.
[Pocket ID](https://github.com/pocket-id/pocket-id) is the recommended
self-host option and ships as a bundled, opt-in compose profile:

```bash
# Off by default -- starts Pocket ID alongside the app
docker compose --profile oidc up -d
```

Pocket ID's own admin UI is published on host port `1411`, bound to
localhost only — open `http://pocket-id.localhost:1411` (the dotted
`*.localhost` name is required: Pocket ID is passkey-based and the hostname
must be a valid WebAuthn RPID; browsers resolve `*.localhost` to 127.0.0.1
on their own) to create clients/users before wiring
`OIDC_*`. Its data lives in its own named volume (`pocket_id_data`),
separate from `nightlio_data` -- enabling/disabling the profile never
touches your Nightlio data.

If you want to enable OIDC (with Pocket ID or any other provider):
1. Create an OIDC client in your provider's admin UI.
2. Set the client's redirect/callback URI to the **api's** callback
   endpoint (this is a server-to-server-style redirect target, not a
   frontend page):
   `http://localhost:5173/api/auth/callback/oidc` (dev, via the frontend's
   nginx `/api/` proxy) or `https://yourdomain.com/api/auth/callback/oidc`
   (prod, same pattern).
3. Update your `.env` file:
   ```bash
   OIDC_ISSUER_URL=http://pocket-id.localhost:1411   # dev (compose network alias; browsers resolve *.localhost themselves)
   OIDC_ISSUER_URL=https://id.yourdomain.com         # prod, public URL
   OIDC_CLIENT_ID=your-client-id
   OIDC_CLIENT_SECRET=your-client-secret
   ```
4. Restart the api: `docker compose up -d`. Once `OIDC_ISSUER_URL` is set,
   `GET /api/config` reports `enable_oidc: true` and credential-free
   single-user login (`POST /api/auth/local/login` with no body) starts
   failing closed with 403 -- local username/password accounts keep working
   alongside OIDC.

### Mood Music Setup (Optional)
If you want to enable mood-based music:
1. Create a free app at [Jamendo API](https://developer.jamendo.com/v3.0)
2. Add your client ID to `.env`:
  ```bash
  ENABLE_MOOD_MUSIC=1
  JAMENDO_CLIENT_ID=your-jamendo-client-id
  ```

## Docker Commands

### Basic Operations
```bash
# Start services
docker compose up -d

# Stop services
docker compose down

# View logs
docker compose logs -f

# View logs for specific service
docker compose logs -f api
docker compose logs -f frontend

# Restart services
docker compose restart

# Update to latest version
git pull
docker compose down
docker compose build --no-cache
docker compose up -d
```

### Data Management
```bash
# Backup your data
docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup alpine tar czf /backup/nightlio-backup.tar.gz -C /data .

# Restore data
docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup alpine tar xzf /backup/nightlio-backup.tar.gz -C /data

# View data volume
docker volume inspect nightlio_nightlio_data
```

## Customization

### Changing Ports
Edit `docker-compose.yml` to change ports:
```yaml
services:
  frontend:
    ports:
      - "8080:8080"  # Frontend on localhost:8080 instead of 5173 (container listens on 8080)
  api:
    ports:
      - "5001:5000"  # API on localhost:5001 instead of 5000
```

### Using External Database
You can mount an external SQLite database:
```yaml
services:
  api:
    volumes:
      - ./path/to/your/nightlio.db:/app/data/nightlio.db
```

### Behind a Reverse Proxy
If running behind nginx, Traefik, or similar:
1. Remove port mappings from `docker-compose.yml`
2. Use the service names (`api`, `frontend`) for internal routing
3. Update the OIDC client's registered callback URL to match your public domain if using OIDC
4. Set `TRUST_PROXY_HEADERS=1` on the api if you want accurate client IPs in rate limiting and a correct `Secure` cookie flag behind the proxy

## Troubleshooting

### Services won't start
```bash
# Check logs
docker compose logs

# Check if ports are already in use
netstat -tlnp | grep -E ':(5173|5000)'

# Clean restart
docker compose down
docker compose up --build
```

### Database issues
```bash
# Reset database (WARNING: deletes all data)
docker compose down
docker volume rm nightlio_nightlio_data
docker compose up -d
```

### Permission issues
```bash
# Fix volume permissions
docker compose exec api chown -R 1000:1000 /app/data
```

## Production Deployment

For production use:

1. **Use a reverse proxy** (nginx, Traefik, Caddy)
2. **Enable HTTPS** with Let's Encrypt
3. **Set strong secrets** in your `.env` file
4. **Backup regularly** using the backup commands above
5. **Monitor logs** with `docker compose logs -f`
6. **Update regularly** for security patches

### Example nginx config
```nginx
server {
    listen 80;
    server_name your-domain.com;
    
    location / {
        proxy_pass http://localhost:5173;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

## Support

If you encounter issues:
1. Check the logs: `docker compose logs`
2. Ensure your `.env` file is configured correctly
3. Make sure ports 5173 and 5000 aren't in use
4. Create an issue on GitHub with your logs

Happy journaling! 🌙
