# Build stage (Debian-based to ensure native binaries resolve on all arches)
FROM node:24-bookworm-slim AS build

WORKDIR /app

# Enable Corepack so the pinned Yarn version is available without a separate install step
RUN corepack enable

# Copy package manifest and lockfile so Yarn resolves arch-specific optional
# dependencies (Rollup) for the build platform
COPY package.json yarn.lock ./

# Install dependencies from the lockfile (Yarn does not have npm's optional-deps
# resolution bug on arm64, so this is safe to keep frozen)
RUN yarn install --frozen-lockfile

# Copy source code
COPY . .

# Build argument for API URL (empty string for relative paths in Docker)
ARG VITE_API_URL=
ENV VITE_API_URL=$VITE_API_URL

# Build the application
RUN yarn build

# Production stage
#
# nginxinc/nginx-unprivileged is the official nginx org's build of the same
# nginx:stable-bookworm image, patched to run the master process itself as
# a non-root user (uid 101, "nginx") instead of starting as root and
# dropping privileges only for worker processes. The plain nginx image had
# no USER directive at all here, so the whole container -- master process
# included -- ran as root: an attacker who reached a nginx vulnerability
# (path traversal, a malformed-request parser bug, etc.) through the
# unauthenticated static file server or /api/ proxy would land as root
# inside the container. It listens on 8080 instead of 80 because binding
# ports below 1024 needs root; the host-side port mapping (docker-compose*)
# is unaffected, only the container-internal port changes.
FROM nginxinc/nginx-unprivileged:stable-bookworm

# Copy built assets from build stage
COPY --from=build /app/dist /usr/share/nginx/html

# Copy nginx configuration
COPY nginx.conf.template /etc/nginx/templates/default.conf.template

# Set configuration env vars that can be passed to nginx and their default values
ENV NGINX_ENVSUBST_FILTER="API_URL|PORT"
ENV API_URL="http://api:5000"
ENV PORT=8080

# Expose the unprivileged port nginx actually listens on (see PORT above).
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD bash -c "exec 3<>/dev/tcp/127.0.0.1/${PORT} && printf 'GET / HTTP/1.0\r\n\r\n' >&3 && grep -q '200 OK' <&3" || exit 1

# Start nginx. No USER directive needed: nginxinc/nginx-unprivileged already
# defaults to a non-root user (uid 101, "nginx") for both the master and
# worker processes.
CMD ["nginx", "-g", "daemon off;"]
