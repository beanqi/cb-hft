use std::collections::HashMap;

use serde::Deserialize;

use crate::event::BalanceEvent;
use crate::types::{AssetId, Qty};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSnapshot {
    balances: Vec<BalanceEvent>,
}

impl AccountSnapshot {
    pub fn new(balances: Vec<BalanceEvent>) -> Self {
        Self { balances }
    }

    pub fn balances(&self) -> &[BalanceEvent] {
        &self.balances
    }
}

#[derive(Default)]
pub struct BalanceBook {
    balances: HashMap<AssetId, BalanceEvent>,
}

impl BalanceBook {
    pub fn apply_snapshot(&mut self, snapshot: AccountSnapshot) {
        self.balances.clear();
        for balance in snapshot.balances {
            self.balances.insert(balance.asset_id.clone(), balance);
        }
    }

    pub fn apply_balance_update(&mut self, balance: BalanceEvent) {
        self.balances.insert(balance.asset_id.clone(), balance);
    }

    pub fn balance(&self, asset_id: &AssetId) -> Option<&BalanceEvent> {
        self.balances.get(asset_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountParseError {
    Json,
    InvalidDecimal,
}

impl core::fmt::Display for AccountParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AccountParseError {}

pub fn parse_rest_accounts_snapshot(
    bytes: &[u8],
    qty_scale: i64,
    recv_ts_ns: u64,
) -> Result<AccountSnapshot, AccountParseError> {
    let accounts: Vec<RestAccount> =
        serde_json::from_slice(bytes).map_err(|_| AccountParseError::Json)?;
    let mut balances = Vec::with_capacity(accounts.len());
    for account in accounts {
        balances.push(BalanceEvent {
            asset_id: AssetId::new(account.currency),
            total: Qty::parse_scaled(account.balance.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            available: Qty::parse_scaled(account.available.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            hold: Qty::parse_scaled(account.hold.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            update_ts_ns: 0,
            recv_ts_ns,
        });
    }
    Ok(AccountSnapshot::new(balances))
}

#[derive(Deserialize)]
struct RestAccount {
    currency: String,
    balance: String,
    hold: String,
    available: String,
}

pub trait RestAccountSnapshotClient {
    type Error;

    fn load_snapshot(&self) -> Result<AccountSnapshot, Self::Error>;
}
