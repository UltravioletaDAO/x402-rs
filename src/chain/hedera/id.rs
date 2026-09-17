//! Numeric entity IDs are interpreted only in Hedera context, never by the
//! global address heuristic (a dotted numeric name is also a legal NEAR name).
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntityId(String);

impl FromStr for EntityId {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.split('.').collect();
        if parts.len() != 3
            || parts.iter().any(|p| {
                p.is_empty()
                    || (p.len() > 1 && p.starts_with('0'))
                    || !p.bytes().all(|c| c.is_ascii_digit())
                    || p.parse::<i64>().is_err()
            })
        {
            return Err(
                "Hedera requires a canonical numeric entity ID; aliases are unsupported".into(),
            );
        }
        Ok(Self(value.to_owned()))
    }
}
impl TryFrom<String> for EntityId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}
impl From<EntityId> for String {
    fn from(id: EntityId) -> Self {
        id.0
    }
}
impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl EntityId {
    pub fn is_hbar(&self) -> bool {
        self.0 == "0.0.0"
    }
    pub fn account(&self) -> Result<hiero_sdk::AccountId, String> {
        if self.is_hbar() {
            return Err("zero account ID".into());
        }
        self.0.parse().map_err(|_| "invalid account ID".into())
    }
}

pub fn valid_transaction_id(value: &str) -> bool {
    let Some((account, timestamp)) = value.split_once('@') else {
        return false;
    };
    let Some((sec, nano)) = timestamp.split_once('.') else {
        return false;
    };
    account.parse::<EntityId>().is_ok_and(|a| !a.is_hbar())
        && !sec.is_empty()
        && sec.bytes().all(|v| v.is_ascii_digit())
        && sec.parse::<i64>().is_ok_and(|s| s > 0)
        && nano.len() == 9
        && nano.bytes().all(|v| v.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entity_ids_are_canonical_and_bounded() {
        for bad in [
            "0.0.-1",
            "00.0.3",
            "0.0.01",
            "0.0.3-x",
            "0.0.9223372036854775808",
            "0.0.alias",
            "0.0. 1",
        ] {
            assert!(bad.parse::<EntityId>().is_err(), "{bad}");
        }
        assert!("0.0.456858".parse::<EntityId>().is_ok());
        assert!("0.0.0".parse::<EntityId>().unwrap().account().is_err());
    }
}
