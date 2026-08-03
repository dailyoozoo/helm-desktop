use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const BUDGET_CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetEnforcementMode {
    HardPreflight,
    Streaming,
    PostFacto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    InputBytes,
    Token,
    CostMicrousd,
    ToolCount,
    RepeatDigest,
    OutputBytes,
    WallClockMs,
    IdleMs,
    ContextRatioPermille,
}

impl BudgetDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputBytes => "input_bytes",
            Self::Token => "token",
            Self::CostMicrousd => "cost_microusd",
            Self::ToolCount => "tool_count",
            Self::RepeatDigest => "repeat_digest",
            Self::OutputBytes => "output_bytes",
            Self::WallClockMs => "wall_clock_ms",
            Self::IdleMs => "idle_ms",
            Self::ContextRatioPermille => "context_ratio_permille",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetLimit {
    pub dimension: BudgetDimension,
    pub limit: u64,
    pub enforcement_mode: BudgetEnforcementMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnBudgetSnapshot {
    pub contract_version: u32,
    pub limits: Vec<BudgetLimit>,
    pub created_at: i64,
}

impl TurnBudgetSnapshot {
    pub fn standard(created_at: i64) -> Self {
        Self {
            contract_version: BUDGET_CONTRACT_VERSION,
            limits: vec![
                limit(
                    BudgetDimension::InputBytes,
                    2 * 1024 * 1024,
                    BudgetEnforcementMode::HardPreflight,
                ),
                limit(
                    BudgetDimension::Token,
                    200_000,
                    BudgetEnforcementMode::PostFacto,
                ),
                limit(
                    BudgetDimension::CostMicrousd,
                    20_000_000,
                    BudgetEnforcementMode::PostFacto,
                ),
                limit(
                    BudgetDimension::ToolCount,
                    128,
                    BudgetEnforcementMode::Streaming,
                ),
                limit(
                    BudgetDimension::RepeatDigest,
                    8,
                    BudgetEnforcementMode::Streaming,
                ),
                limit(
                    BudgetDimension::OutputBytes,
                    16 * 1024 * 1024,
                    BudgetEnforcementMode::Streaming,
                ),
                limit(
                    BudgetDimension::WallClockMs,
                    60 * 60 * 1000,
                    BudgetEnforcementMode::Streaming,
                ),
                limit(
                    BudgetDimension::IdleMs,
                    5 * 60 * 1000,
                    BudgetEnforcementMode::Streaming,
                ),
                limit(
                    BudgetDimension::ContextRatioPermille,
                    950,
                    BudgetEnforcementMode::PostFacto,
                ),
            ],
            created_at,
        }
    }

    pub fn limit(&self, dimension: BudgetDimension) -> Option<&BudgetLimit> {
        self.limits
            .iter()
            .find(|limit| limit.dimension == dimension)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != BUDGET_CONTRACT_VERSION {
            return Err("不支持的 TurnBudgetSnapshot 契约版本".to_string());
        }
        let mut seen = HashMap::new();
        for limit in &self.limits {
            if limit.limit == 0 {
                return Err(format!(
                    "预算维度 {} 的上限必须大于 0",
                    limit.dimension.as_str()
                ));
            }
            if seen.insert(limit.dimension, ()).is_some() {
                return Err(format!("预算维度 {} 重复", limit.dimension.as_str()));
            }
        }
        for dimension in [
            BudgetDimension::InputBytes,
            BudgetDimension::Token,
            BudgetDimension::CostMicrousd,
            BudgetDimension::ToolCount,
            BudgetDimension::RepeatDigest,
            BudgetDimension::OutputBytes,
            BudgetDimension::WallClockMs,
            BudgetDimension::IdleMs,
            BudgetDimension::ContextRatioPermille,
        ] {
            if !seen.contains_key(&dimension) {
                return Err(format!("预算快照缺少维度 {}", dimension.as_str()));
            }
        }
        Ok(())
    }

    pub fn enforce_input_bytes(&self, bytes: usize) -> Result<(), String> {
        let limit = self
            .limit(BudgetDimension::InputBytes)
            .ok_or_else(|| "预算快照缺少 input_bytes".to_string())?;
        if bytes as u64 > limit.limit {
            return Err(format!(
                "[budget_input_bytes_exceeded] 输入 {} 字节，硬上限 {} 字节",
                bytes, limit.limit
            ));
        }
        Ok(())
    }
}

fn limit(
    dimension: BudgetDimension,
    limit: u64,
    enforcement_mode: BudgetEnforcementMode,
) -> BudgetLimit {
    BudgetLimit {
        dimension,
        limit,
        enforcement_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_snapshot_freezes_every_required_dimension_and_honest_modes() {
        let snapshot = TurnBudgetSnapshot::standard(42);
        snapshot.validate().unwrap();
        assert_eq!(snapshot.limits.len(), 9);
        assert_eq!(
            snapshot
                .limit(BudgetDimension::Token)
                .unwrap()
                .enforcement_mode,
            BudgetEnforcementMode::PostFacto
        );
        assert_eq!(
            snapshot
                .limit(BudgetDimension::ToolCount)
                .unwrap()
                .enforcement_mode,
            BudgetEnforcementMode::Streaming
        );
    }

    #[test]
    fn input_size_is_a_real_preflight_limit() {
        let snapshot = TurnBudgetSnapshot::standard(42);
        let limit = snapshot.limit(BudgetDimension::InputBytes).unwrap().limit;
        snapshot.enforce_input_bytes(limit as usize).unwrap();
        assert!(snapshot.enforce_input_bytes(limit as usize + 1).is_err());
    }
}
