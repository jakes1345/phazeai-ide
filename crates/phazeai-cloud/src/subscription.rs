use serde::{Deserialize, Serialize};

/// PhazeAI subscription tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Tier {
    /// Self-hosted / bring-your-own-key. Free, no cloud account needed.
    #[default]
    SelfHosted,
    /// PhazeAI Cloud — hosted models, ~$20/mo.
    Cloud,
    /// Team tier — collaboration features, ~$50/seat/mo.
    Team,
    /// Enterprise — on-premise, SSO, SLA.
    Enterprise,
}

impl Tier {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::SelfHosted  => "Self-Hosted (Free)",
            Self::Cloud       => "PhazeAI Cloud",
            Self::Team        => "Team",
            Self::Enterprise  => "Enterprise",
        }
    }

    pub fn monthly_price_usd(&self) -> Option<u32> {
        match self {
            Self::SelfHosted  => None,
            Self::Cloud       => Some(20),
            Self::Team        => Some(50),
            Self::Enterprise  => None, // custom
        }
    }

    pub fn has_cloud_ai(&self) -> bool {
        matches!(self, Self::Cloud | Self::Team | Self::Enterprise)
    }

    pub fn has_team_features(&self) -> bool {
        matches!(self, Self::Team | Self::Enterprise)
    }
}
