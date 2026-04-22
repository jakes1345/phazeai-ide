# phazeai-cloud

Optional cloud client for PhazeAI Cloud — a paid tier providing hosted AI models, authentication, team collaboration, and managed infrastructure.

## Features

- **Authentication**: OAuth2 flow with secure token storage
- **Subscription Management**: Support for SelfHosted, Cloud, Team, and Enterprise tiers
- **Model Proxying**: Use PhazeAI-hosted Claude, GPT-4, and custom models via simple API key
- **Team Collaboration**: Shared workspaces, conversation history, usage analytics (future)
- **License Enforcement**: Per-tier feature gates and usage quotas

## Status

This crate is a skeleton for future cloud features. The open-source IDE works fully without it — all core functionality is in `phazeai-core` and `phazeai-ui`.

## Dependencies

- `phazeai-core` — Shared agent and LLM infrastructure
- Standard async stack: tokio, reqwest, serde, async-trait

## Usage

Cloud features are optional and auto-disabled in self-hosted deployments. When enabled:

```rust
use phazeai_cloud::CloudClient;

let client = CloudClient::new(api_key)?;
let user = client.get_user().await?;
```

## License

MIT — See LICENSE in repository root
