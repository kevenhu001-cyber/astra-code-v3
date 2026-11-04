# Astra website and model routing

`astra-auth` serves the public site, account pages, administrator pages, and
the CLI's OpenAI-compatible `/v1` gateway.

## Set up the administrator

Set `ASTRA_AUTH_ADMIN_EMAIL` on the `astra-auth` service, then register and
verify that email address on the site. The matching account is promoted when
the service starts or when its verification completes. Only that configured
email remains an administrator after a restart.

After signing in with the configured email, open **Admin tools** from the
account page to configure model routes and add account credit. The old `/admin`
URL redirects into that account-page section. No administrator password is
built into the service.

## Configure model routes

Each route has a CLI model id (generated from its display name when omitted),
the display name shown to CLI users, an upstream protocol, API base URL, model
id, context window, API key, and input/output prices per million tokens. The
API key stays on the server and is never returned by the admin API. Supported
upstream protocols are OpenAI-compatible Chat Completions, OpenAI Responses,
and Anthropic Messages. Each uses its native gateway endpoint:
`/v1/chat/completions`, `/v1/responses`, or `/v1/messages`. The CLI follows the
protocol advertised for each model in the catalog. Requests, responses, and
streaming events are not converted between protocols; only the public model
id is mapped to the provider's model id. A protocol mismatch is rejected.

Routes and account balances are stored in the service's `auth.json`; writes
replace the file atomically. On Unix, new state files are written with mode
`0600`.

The account page shows the available USD balance and recent ledger entries.
Administrators add credit manually from the account-page admin section; model
requests are charged from provider usage using the configured route prices. A
balance is not recharged automatically.
