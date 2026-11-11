# syntax=docker/dockerfile:1
FROM node:24.22.2-bookworm-slim@sha256:01d566e8d94d8c619591d78652babfccd3c6992a3dffa9ed5ab39e11cf6c70e9 AS dependencies
RUN npm install --global pnpm@12.18.0 && mkdir /app && chown node:node /app
WORKDIR /app
USER node
COPY --chown=node:node package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY --chown=node:node apps/frontend/package.json apps/frontend/package.json
RUN pnpm install --frozen-lockfile
COPY --chown=node:node apps/frontend ./apps/frontend

FROM dependencies AS development
CMD ["pnpm", "--filter", "@apodect/frontend", "dev"]

FROM dependencies AS build
RUN pnpm --filter @apodect/frontend build

FROM caddy:2.11.9-alpine@sha256:0bbb055bda6793c999bde78b1b5ce3e89f829fb031af2cf50056ea1246559fee AS production
COPY infra/Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/apps/frontend/dist /srv
