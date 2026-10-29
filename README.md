# Public Note Board

A shared sticky-note board that opens a new board each day using Korea Standard Time (KST). Anyone can post notes to today's board and browse read-only archives dated 2026-10-14 through today. The default board remains today's board; dates before the archive start and future dates cannot be selected for browsing. Notes are limited to 10 lines and 250 characters. Leading blank lines and trailing whitespace are removed, but indentation is kept, and each note card grows to show the whole note.

## Structure

- `src/lib.rs`: Rust Cloudflare Worker API
- `public/`: static Cloudflare Pages frontend
- `migrations/`: Cloudflare D1 schema migrations
- `wrangler.toml`: Worker, D1, and posting-rate-limit configuration

## Local development

Required tools: Rust, the `wasm32-unknown-unknown` target, Node.js/npm, Wrangler, and `worker-build`.

```sh
rustup target add wasm32-unknown-unknown
cargo install worker-build
npm install
npm run migrate:local
```

Run these commands in separate terminals:

```sh
npm run dev:worker
npm run dev:pages
```

The Pages development server uses `http://localhost:8788`; the API uses `http://localhost:8787`. The local API URL is configured in `public/config.js`.

## Manual administrator access

There is no login form or login endpoint. Generate a high-entropy token with `openssl rand -hex 32`.

- For local development, place `ADMIN_TOKEN=<token>` in the ignored `.dev.vars` file.
- For Cloudflare, set it with `npx wrangler secret put ADMIN_TOKEN`.
- In browser developer tools, manually add a cookie for the **Worker API origin**: name `admin_session`, value exactly the token. Reload the Pages site to activate admin controls.
- For the production same-origin setup at `https://note.random6.xyz`, use `HttpOnly`, `Secure`, `SameSite=Lax`, and `Path=/`. For a cross-site Pages/Worker setup, use `SameSite=None` instead. For local `http://localhost`, use `HttpOnly`, `SameSite=Lax`, and `Path=/` without `Secure`.

With a valid cookie, each note has an `×` delete button and a `Reset` button deletes all notes from today (KST). Rotating `ADMIN_TOKEN` revokes existing cookies. The API exposes `GET /api/admin/session`, `DELETE /api/admin/notes/{id}`, and `DELETE /api/admin/notes/today`.

Verification commands:

```sh
npm run fmt
npm test
npm run build:worker
```

## Cloudflare deployment

The production setup serves both the Pages site and the Worker API under `https://note.random6.xyz`. The Worker route handles `note.random6.xyz/api/*`; other paths are served by Pages. This requires an active Cloudflare zone for `random6.xyz` and a proxied DNS record for `note.random6.xyz`.

1. Create the Pages project `rand-note` and attach `note.random6.xyz` as its custom domain in Cloudflare Pages.
2. Create the D1 database with `npx wrangler d1 create rand-note`, then replace `database_id` in `wrangler.toml` with the returned ID. Make sure the Rate Limiting `namespace_id` is unique in your account.
3. Confirm `ALLOWED_ORIGIN` is `https://note.random6.xyz` and that the Worker route in `wrangler.toml` is `note.random6.xyz/api/*` in the `random6.xyz` zone.
4. To enable administrator controls, set the Worker secret `ADMIN_TOKEN` with `npx wrangler secret put ADMIN_TOKEN`. Never commit its value.
5. Apply the remote D1 migration and deploy both services:

   ```sh
   npx wrangler d1 migrations apply rand-note --remote
   npm run deploy:worker
   npm run deploy:pages
   ```

The frontend API base URL is configured in `public/config.js` as `https://note.random6.xyz`. The GitHub Actions workflow in `.github/workflows/deploy-cloudflare.yml` repeats the migration and both deployments on pushes to `main`; configure `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` as GitHub Actions repository secrets first.

The Worker uses Cloudflare's `CF-Connecting-IP` value to limit each IP to five posts per minute. The application database does not store IP addresses. Cloudflare aggregates Rate Limiting binding counters per data-center location. Operators must remove inappropriate notes manually through the D1 management tools.
