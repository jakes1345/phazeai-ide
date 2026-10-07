> **Status:** detailed Cloud backlog. The canonical, up-to-date checklist is [`TODO.md`](TODO.md) — check items off there first.

# PhazeAI Cloud — Phase 3 Master Feature List
> 200+ items. Modeled on Gitpod/Codespaces/Codeium + what Lapdev originally was before K8s pivot.
> phazeai-cloud crate is the backend client; we'll need a real backend service too.
> Status: `[ ]` = not started · `[~]` = in progress · `[x]` = done

---

## 🔴 P0 — Core Platform (Without This There Is No Cloud)

### Authentication & Accounts
- [ ] **Email + password signup** — register at app.phazeai.com with email verification
- [ ] **GitHub OAuth** — "Sign in with GitHub" for instant onboarding
- [ ] **Google OAuth** — "Sign in with Google"
- [ ] **GitLab OAuth** — "Sign in with GitLab"
- [ ] **API token generation** — create named tokens with configurable scopes
- [ ] **API token revocation** — revoke individual tokens from dashboard
- [ ] **Token expiry** — configurable expiry (never / 30d / 90d / 1yr)
- [ ] **Session management** — list active sessions, revoke individual sessions
- [ ] **Password reset** — email-based forgot-password flow
- [ ] **Email change** — verify new email before updating
- [ ] **Account deletion** — GDPR-compliant self-service account deletion
- [ ] **2FA / TOTP** — time-based one-time password (Google Authenticator compatible)
- [ ] **2FA recovery codes** — one-time backup codes for 2FA recovery
- [ ] **JWT-based auth** — short-lived access tokens + long-lived refresh tokens
- [ ] **Rate limiting on auth endpoints** — brute-force protection

### Billing & Subscriptions
- [ ] **Free tier** — bring-your-own-key, unlimited local usage, no account needed
- [ ] **Pro tier ($20/mo)** — hosted models, cloud sync, no API key required
- [ ] **Team tier ($50/seat/mo)** — shared context, pair programming, audit logs
- [ ] **Enterprise tier** — custom pricing, on-premise, SSO, SLA
- [ ] **Stripe integration** — card payments, invoicing, subscription management
- [ ] **Billing dashboard** — view current plan, next invoice date, payment method
- [ ] **Upgrade/downgrade** — self-service plan changes, prorated billing
- [ ] **Usage tracking** — track tokens consumed per user per billing period
- [ ] **Usage limits** — hard/soft limits per tier; notify at 80% / block at 100%
- [ ] **Credit top-up** — buy additional token credits without upgrading plan
- [ ] **Invoice history** — download past invoices as PDF
- [ ] **Coupon codes** — apply discount codes at checkout
- [ ] **Trial period** — 14-day Pro trial for new accounts
- [ ] **Cancellation flow** — cancel subscription, keep access until period end
- [ ] **Refund policy** — automated refund for first 7 days

### Hosted Model Proxy
- [ ] **LlmClient trait impl for CloudClient** — wire `phazeai-cloud` into phazeai-core's provider system
- [ ] **OpenRouter backend** — route requests through OpenRouter so we don't manage GPU infra
- [ ] **Claude passthrough** — route Anthropic requests with our billing key
- [ ] **GPT-4o passthrough** — route OpenAI requests
- [ ] **Gemini passthrough** — route Google Gemini requests
- [ ] **Streaming support** — SSE streaming from cloud proxy to IDE client
- [ ] **Token counting** — count and bill input + output tokens per request
- [ ] **Cost allocation** — per-request cost tracked to user account
- [ ] **Model selection UI** — settings panel shows cloud-available models based on tier
- [ ] **Fallback routing** — if primary provider is down, route to backup
- [ ] **Request logging** — log all requests for billing audit (NOT content by default)
- [ ] **Response caching** — cache identical prompts for N minutes to reduce costs
- [ ] **Latency monitoring** — track p50/p95/p99 latency per provider
- [ ] **Rate limit per user** — prevent one user from monopolizing shared infra

### Cloud Settings Sync
- [ ] **Settings sync** — push/pull `settings.toml` to cloud on change
- [ ] **Keybindings sync** — sync custom keybindings across devices
- [ ] **Theme sync** — sync active theme choice across devices
- [ ] **Extension/plugin sync** — sync installed extensions list
- [ ] **Snippet sync** — sync user-defined snippets
- [ ] **Conflict resolution** — last-write-wins with "resolve conflict" dialog on clash
- [ ] **Sync toggle** — per-category enable/disable (sync settings but not keybindings, etc.)
- [ ] **Device list** — show all synced devices, remove specific device
- [ ] **Sync history** — last 10 versions of each synced file (rollback support)

---

## 🟠 P1 — Makes It Actually Useful

### Cloud Workspaces (Hosted Dev Environments)
- [ ] **Workspace creation** — spin up ephemeral cloud dev environment from git repo URL
- [ ] **Workspace templates** — predefined environments (Rust, Node, Python, Go, Full-stack)
- [ ] **Custom Docker images** — specify base image in `phazeai.yaml` or `devcontainer.json`
- [ ] **devcontainer.json support** — read and apply VS Code-compatible devcontainer config
- [ ] **dotfiles repository** — link personal dotfiles repo, auto-applied on workspace start
- [ ] **Workspace start** — start stopped workspace; restore filesystem state
- [ ] **Workspace stop** — gracefully stop running workspace to save credits
- [ ] **Workspace delete** — permanently delete workspace and its storage
- [ ] **Workspace list** — dashboard showing all workspaces with status/last-active
- [ ] **Workspace rename** — give workspace a human-readable name
- [ ] **Workspace clone** — duplicate an existing workspace
- [ ] **Auto-stop** — stop workspace after configurable idle timeout (default 30 min)
- [ ] **Auto-start on git push** — optionally trigger workspace start on branch push
- [ ] **Prebuild** — pre-build workspace image so open is instant (run tasks on push)
- [ ] **Persistent storage** — `/workspace` directory persists across stop/start cycles
- [ ] **Ephemeral storage** — `/tmp` reset each restart; clear separation from persistent
- [ ] **Resource classes** — Small (2 CPU/4GB), Medium (4 CPU/8GB), Large (8 CPU/16GB)
- [ ] **GPU workspace** — optional NVIDIA GPU attachment for ML workloads
- [ ] **Storage size tiers** — 10GB / 30GB / 100GB persistent storage options
- [ ] **Workspace sharing** — generate shareable link to read-only view of workspace
- [ ] **Workspace environment variables** — set env vars that persist in workspace
- [ ] **Port forwarding** — expose workspace ports as HTTPS preview URLs
- [ ] **Port visibility** — public / team / private port access controls
- [ ] **Preview URL** — `https://{port}-{workspace-id}.phazeai.app` URLs
- [ ] **SSH access** — `ssh workspace-id@ssh.phazeai.com` to any running workspace
- [ ] **SSH key management** — add/remove SSH public keys from account dashboard
- [ ] **IDE connection** — phazeai-ui connects to cloud workspace via SSH/proxy
- [ ] **Web IDE fallback** — browser-based terminal + editor for workspace access without local IDE
- [ ] **Workspace logs** — view stdout of startup tasks and background processes
- [ ] **Startup tasks** — run `cargo build` / `npm install` automatically on workspace start
- [ ] **Workspace health check** — ping endpoint to verify workspace is alive
- [ ] **Workspace metrics** — CPU / RAM / disk usage in workspace dashboard

### Git Integration (Cloud)
- [ ] **GitHub App** — install PhazeAI GitHub App for repo access without PAT
- [ ] **GitLab integration** — OAuth-based GitLab repo access
- [ ] **Bitbucket integration** — OAuth-based Bitbucket repo access
- [ ] **Open in PhazeAI button** — browser extension that adds button to GitHub/GitLab repos
- [ ] **gitpod.yml / phazeai.yaml** — workspace config file in repo root
- [ ] **PR preview workspaces** — auto-create workspace for each opened pull request
- [ ] **Commit from workspace** — full git operations inside cloud workspace

### Team & Collaboration
- [ ] **Organization creation** — create named org, invite members
- [ ] **Member roles** — Owner / Admin / Member / Guest roles
- [ ] **Seat management** — add/remove seats from billing dashboard
- [ ] **Shared workspace templates** — org admins publish templates for team members
- [ ] **Team usage dashboard** — admin sees usage per member
- [ ] **Shared environment variables** — org-level secrets available to all team workspaces
- [ ] **Live share** — real-time collaborative editing in a shared workspace
- [ ] **Workspace observer mode** — read-only view of teammate's workspace terminal + editor
- [ ] **Pair programming** — two users share cursor in same editor, each can type
- [ ] **Chat in workspace** — in-workspace text chat with collaborators (no Slack needed)
- [ ] **Mentions** — `@username` in workspace chat sends notification
- [ ] **Emoji reactions** — react to workspace chat messages
- [ ] **Shared AI context** — team shares conversation history and project context
- [ ] **Review mode** — reviewer joins workspace read-only to inspect and comment on code

### Security
- [ ] **Workspace network isolation** — each workspace in its own VPC/network namespace
- [ ] **Secrets manager** — store API keys as encrypted secrets, inject at workspace start
- [ ] **Secret rotation** — UI to update secret value across all workspaces
- [ ] **Audit log** — every auth event, workspace action, settings change logged with timestamp
- [ ] **RBAC** — fine-grained permissions per resource per role
- [ ] **IP allowlist** — restrict workspace access to specific IP ranges (Enterprise)
- [ ] **SOC 2 Type II** — compliance documentation and controls
- [ ] **Data residency** — choose US / EU region for workspace data storage
- [ ] **Encryption at rest** — workspace storage AES-256 encrypted
- [ ] **Encryption in transit** — all traffic TLS 1.3
- [ ] **Vulnerability scanning** — scan workspace Docker images for known CVEs
- [ ] **Dependency audit** — run `cargo audit` / `npm audit` in prebuild
- [ ] **Signed workspace images** — verify image integrity before mounting

---

## 🟡 P2 — Growth and Retention Features

### Developer Experience
- [ ] **CLI: `phaze` tool** — `phaze open github.com/user/repo` spins up workspace
- [ ] **CLI: `phaze workspace list`** — list workspaces from terminal
- [ ] **CLI: `phaze workspace start/stop`** — control workspaces from CLI
- [ ] **CLI: `phaze ssh`** — shorthand for SSHing into workspace by name
- [ ] **CLI: `phaze env set KEY=VALUE`** — set workspace env var from CLI
- [ ] **CLI: `phaze port forward`** — forward remote port to localhost
- [ ] **CLI: `phaze logs`** — stream workspace startup logs
- [ ] **CLI: `phaze open`** — open workspace in local phazeai-ui IDE
- [ ] **Browser extension** — adds "Open in PhazeAI" to GitHub/GitLab/Bitbucket
- [ ] **VS Code extension** — allows VS Code users to connect to PhazeAI workspaces
- [ ] **JetBrains Gateway plugin** — JetBrains IDE connection to cloud workspaces
- [ ] **Neovim plugin** — `phazeai.nvim` for connecting to workspaces from Neovim

### Notifications
- [ ] **Email notifications** — workspace ready, prebuild complete, usage limit warnings
- [ ] **In-app notifications** — notification bell in IDE with unread count
- [ ] **Webhook support** — POST to user-configured URL on workspace events
- [ ] **Slack integration** — post workspace ready / stopped events to Slack channel
- [ ] **GitHub status checks** — post PR preview workspace URL as GitHub status

### API
- [ ] **REST API v1** — full API for all workspace/account operations
- [ ] **API documentation** — OpenAPI 3.0 spec, auto-generated from routes
- [ ] **API playground** — interactive API docs with try-it-now UI
- [ ] **SDK: Rust** — `phazeai-sdk` crate for building on top of the API
- [ ] **SDK: TypeScript** — `@phazeai/sdk` npm package
- [ ] **SDK: Python** — `phazeai` PyPI package
- [ ] **Webhook signature verification** — HMAC-SHA256 signed webhook payloads
- [ ] **API versioning** — stable `/v1` with deprecation notices for breaking changes
- [ ] **Rate limiting** — per-token rate limits with `Retry-After` headers
- [ ] **GraphQL API** (optional) — alternative query API for dashboard use cases

### Dashboard & Admin
- [ ] **Web dashboard** — app.phazeai.com: workspace management, account, billing
- [ ] **Workspace status indicators** — running (green) / stopped (gray) / starting (spinner)
- [ ] **Usage graphs** — daily/weekly token usage, workspace hours, cost charts
- [ ] **Cost breakdown** — cost per workspace per day
- [ ] **Admin panel** — super-admin view of all users, orgs, revenue (internal)
- [ ] **Feature flags** — per-user/org feature rollouts without deploys
- [ ] **Support ticket integration** — in-dashboard "Contact Support" → Intercom/Linear
- [ ] **Changelog page** — what's new in each release, linked from IDE notification
- [ ] **Status page** — public uptime page at status.phazeai.com
- [ ] **Incident notifications** — email/banner when there's an outage

### Infrastructure
- [ ] **Multi-region deployment** — US-East, US-West, EU-West, AP-Southeast regions
- [ ] **Region selection** — user picks preferred region for workspaces
- [ ] **Kubernetes orchestration** — workspace pods on managed K8s (EKS / GKE / AKS)
- [ ] **Workspace pod resource limits** — CPU/RAM limits enforced at K8s level
- [ ] **Auto-scaling** — scale workspace node pool up/down with demand
- [ ] **Workspace image registry** — private registry for custom workspace images
- [ ] **Image caching** — cache base images on nodes to speed up workspace start
- [ ] **Workspace DNS** — each workspace gets stable internal DNS name
- [ ] **Ingress controller** — Nginx/Traefik routing preview URLs to correct workspace pods
- [ ] **TLS certificate management** — cert-manager auto-issues Let's Encrypt certs
- [ ] **Storage provisioner** — dynamic PVC provisioning per workspace (EBS / GCE PD)
- [ ] **Workspace backup** — nightly snapshot of persistent storage to object storage
- [ ] **Workspace restore** — restore workspace from backup snapshot
- [ ] **Health checks** — liveness/readiness probes on workspace containers
- [ ] **Workspace logs aggregation** — centralized logging (Loki/CloudWatch)
- [ ] **Metrics collection** — Prometheus metrics for all workspace pods
- [ ] **Distributed tracing** — OpenTelemetry traces for API requests
- [ ] **Alerting** — PagerDuty/OpsGenie alerts on SLO violations

### Self-Hosted / Enterprise
- [ ] **Self-hosted deployment** — Helm chart to deploy entire PhazeAI Cloud on own K8s
- [ ] **Air-gap support** — fully offline self-hosted with bundled images
- [ ] **SAML SSO** — SAML 2.0 integration (Okta, Azure AD, Ping)
- [ ] **LDAP / Active Directory** — enterprise user directory integration
- [ ] **Custom domain** — bring your own domain (ide.yourcompany.com)
- [ ] **Custom CA** — trust internal certificate authority in workspaces
- [ ] **License key management** — offline license validation for air-gap installs
- [ ] **SLA** — 99.9% uptime SLA with credits for downtime
- [ ] **Dedicated cluster** — isolated K8s cluster for enterprise customers
- [ ] **Private network access** — workspace can reach private VPC resources (VPN/peering)
- [ ] **Compliance exports** — export audit logs to SIEM (Splunk, Datadog)
- [ ] **Data export** — export all user data on request (GDPR Art. 20)
- [ ] **Right to erasure** — delete all user data on request (GDPR Art. 17)

---

## 🔵 P3 — Platform Extension & Ecosystem

### AI Platform Layer
- [ ] **Shared team memory** — team's AI chat history and project context stored in cloud
- [ ] **Cross-workspace context** — AI has access to context from all org repos
- [ ] **Embeddings indexing** — semantic search over org codebase via vector DB
- [ ] **Code search API** — `GET /api/search?q=function+name` returns semantic matches
- [ ] **AI model fine-tuning** — fine-tune on org's codebase for better completions (Enterprise)
- [ ] **Prompt library** — save and share useful AI prompts across team
- [ ] **AI usage analytics** — which prompts get accepted, which get rejected
- [ ] **Plugin marketplace** — community plugins hosted on cloud, one-click install
- [ ] **Plugin revenue sharing** — plugin authors get % of usage revenue

### Collaboration Network
- [ ] **Public profiles** — optional public page showing open-source contributions
- [ ] **Workspace templates marketplace** — share workspace configs with community
- [ ] **Snippet sharing** — publish code snippets with public URL
- [ ] **Code review rooms** — create ephemeral shared session for async code review
- [ ] **Screenshare** — WebRTC-based screen share within a workspace session
- [ ] **Voice chat** — in-workspace voice channel for pair programming

### Analytics & Insights
- [ ] **Coding activity heatmap** — GitHub-style contribution graph for coding time
- [ ] **Language breakdown** — pie chart of time spent per language
- [ ] **Productivity metrics** — lines written, PRs opened, AI assists accepted
- [ ] **Team velocity dashboard** — aggregate team coding metrics for engineering managers
- [ ] **AI acceptance rate** — track what % of AI suggestions are accepted per dev
- [ ] **Workspace utilization** — which workspaces are heavily/lightly used

### Marketplace & Ecosystem
- [ ] **Template marketplace** — browse community workspace templates
- [ ] **Plugin registry** — hosted plugin distribution (WASM bundles)
- [ ] **Plugin versioning** — semantic versioning, pin to specific version
- [ ] **Plugin sandboxing** — WASM runtime limits (CPU time, memory, network access)
- [ ] **Plugin revenue** — paid plugins with Stripe Connect for author payouts
- [ ] **Theme gallery** — browse and install community themes

### Mobile & Accessibility
- [ ] **iPad companion app** — read code, review PRs, chat with AI on iPad
- [ ] **Android app** — same as iPad, for Android tablets
- [ ] **Mobile notifications** — push notifications for build complete, PR review requested
- [ ] **PWA** — phazeai.com installable as Progressive Web App
- [ ] **Web terminal** — browser-based terminal for quick workspace access on any device
