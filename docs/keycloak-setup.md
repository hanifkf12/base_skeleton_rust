# Keycloak Setup Guide

This guide configures local Keycloak for this service's OIDC-protected user API. The Rust service is a resource server: it validates Keycloak-issued JWT access tokens, but does not implement login pages, store passwords, or exchange authorization codes.

The examples use these names:

| Setting | Example value |
| --- | --- |
| Keycloak realm | `demo` |
| API audience/client | `base-skeleton-api` |
| Postman login client | `postman-local` |
| Required scopes | `users:read`, `users:write` |
| Local Keycloak URL | `http://localhost:8081` |

## 1. Run Keycloak locally with Docker

Start a development-only Keycloak instance:

```bash
docker run --name keycloak-dev \
  --publish 8081:8080 \
  --env KC_BOOTSTRAP_ADMIN_USERNAME=admin \
  --env KC_BOOTSTRAP_ADMIN_PASSWORD=change-me-local-only \
  quay.io/keycloak/keycloak:latest \
  start-dev
```

Open <http://localhost:8081>, then sign in to the administration console with the bootstrap credentials above. Use a strong unique password outside local development, pin a Keycloak image version, configure HTTPS, and use a persistent database before deploying Keycloak to production.

## 2. Create the realm and API audience

1. In the Keycloak administration console, create a realm named `demo`.
2. In that realm, create an OpenID Connect client named `base-skeleton-api`.
3. This client represents the API resource and supplies the audience value. It does not need to be used for interactive login in this guide.

The issuer and discovery URLs are:

```text
Issuer:    http://localhost:8081/realms/demo
Discovery: http://localhost:8081/realms/demo/.well-known/openid-configuration
```

The service requires `OIDC_ISSUER_URL` to match the discovery document's `issuer` value; a trailing slash in the configured value is tolerated. Do not mix `localhost` and `127.0.0.1`.

## 3. Create scopes and authorize a user

Create two OpenID Connect client scopes:

1. Go to **Client scopes** and create `users:read`.
2. Enable **Include in token scope**.
3. Repeat for `users:write`.

For a local demonstration, link both scopes to the Postman client as **Default** client scopes. Default scopes are included automatically in issued tokens.

For controlled access, create corresponding realm roles, for example `users:read` and `users:write`, assign them to the intended users or groups, and add the matching role as a role scope mapping on each client scope. Keycloak then applies a scope only to users authorized for its mapped role.

## 4. Create the Postman login client

Create another OpenID Connect client named `postman-local` for interactive Postman login:

1. Enable **Standard flow**.
2. Leave **Client authentication** off for an authorization-code-with-PKCE local client.
3. Set **Valid redirect URIs** to:

   ```text
   https://oauth.pstmn.io/v1/callback
   ```

4. Link `users:read` and `users:write` to this client as **Default** scopes for the local demonstration.

For machine-to-machine calls instead, create a confidential client, enable **Client authentication** and **Service account roles**, then use the client-credentials grant. Do not use a browser-login client secret in a desktop client or front-end application.

## 5. Add the API audience to access tokens

The Rust API validates the JWT `aud` claim. Keycloak access tokens often contain only `account` by default, so add the API audience explicitly:

1. Go to **Clients → postman-local → Client scopes**.
2. Open the dedicated client scope, usually named `postman-local-dedicated`.
3. Go to **Mappers → Add mapper → By configuration → Audience**.
4. Configure the mapper:

   | Field | Value |
   | --- | --- |
   | Name | `base-skeleton-api-audience` |
   | Included Client Audience | `base-skeleton-api` |
   | Add to access token | enabled |

5. Save it.

Keycloak's official documentation describes client scopes and audience protocol mappers in its [Server Administration Guide](https://www.keycloak.org/docs/latest/server_admin/).

## 6. Configure and start the Rust service

Copy the environment file and set the Keycloak values:

```bash
cp .env.example .env
```

```dotenv
OIDC_ISSUER_URL=http://localhost:8081/realms/demo
OIDC_AUDIENCE=base-skeleton-api
OIDC_ALLOWED_ALGORITHMS=RS256
OIDC_ALLOW_INSECURE_HTTP=true
```

Then start the service:

```bash
docker compose up -d
cargo run -- db migrate
cargo run -- http
```

At startup, the service loads Keycloak discovery metadata and signing keys. If startup fails, confirm that the issuer URL is reachable, exact, and that Keycloak publishes an asymmetric signing key such as `RS256`.

## 7. Get and use a token in Postman

Create a request to `GET http://localhost:3000/api/v1/users`.

In Postman's **Authorization** tab:

| Field | Value |
| --- | --- |
| Type | `OAuth 2.0` |
| Grant Type | `Authorization Code (With PKCE)` |
| Callback URL | `https://oauth.pstmn.io/v1/callback` |
| Auth URL | `http://localhost:8081/realms/demo/protocol/openid-connect/auth` |
| Access Token URL | `http://localhost:8081/realms/demo/protocol/openid-connect/token` |
| Client ID | `postman-local` |
| Scope | `openid` |

Click **Get New Access Token**, authenticate in Keycloak, click **Use Token**, then send the request. Postman sends:

```http
Authorization: Bearer <access_token>
```

Use the `access_token` only. Do not send the `id_token` or `refresh_token` to this API.

Because `users:read` and `users:write` are Default client scopes in this local setup, Postman requests only `openid`. If you make them Optional instead, request them as a space-separated scope value:

```text
openid users:read users:write
```

## 8. Check the token claims

Inspect the token payload using Postman's token viewer or Keycloak's **Client scopes → Evaluate** screen. Do not paste production tokens into public JWT tools.

The access token must include values equivalent to:

```json
{
  "iss": "http://localhost:8081/realms/demo",
  "aud": ["account", "base-skeleton-api"],
  "sub": "keycloak-user-id",
  "scope": "openid profile email users:read users:write"
}
```

`aud` can be a string or array, but it must contain `base-skeleton-api`. The API accepts `GET` and `HEAD` with `users:read`; `POST`, `PUT`, and `DELETE` require `users:write`.

## Troubleshooting

| Result | Cause | Fix |
| --- | --- | --- |
| Keycloak redirects with `invalid_scope` | A requested scope is not linked to the client as Optional, or the scope name is incorrect. | Use Default scopes and request only `openid`, or link the scopes as Optional before requesting them. |
| API returns `401 unauthorized` | The access token is malformed, expired, signed by an unknown key, or has mismatched `iss`/`aud`. | Use the `access_token`, configure the Audience mapper, and ensure `OIDC_ISSUER_URL` exactly matches `iss`. |
| API returns `403 insufficient_scope` | The access token is valid but lacks `users:read` or `users:write`. | Link/assign the client scope or mapped role, then obtain a fresh token. |
| API returns `503 authentication_unavailable` | The API needed to refresh Keycloak JWKS but Keycloak was unreachable. | Restore Keycloak connectivity and retry. |
| Service runs in Docker but cannot reach Keycloak | `localhost` inside the service container is not the host machine or Keycloak container. | Use a hostname reachable from both containers and configure Keycloak's issuer/hostname consistently. |

Never commit client secrets or tokens. If a token or refresh token is exposed, revoke or log out its Keycloak session and obtain a new one.
