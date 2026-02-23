//! Budget management

use std::sync::atomic::{AtomicU64, Ordering};
use rust_decimal::Decimal;

/// Budget manager for API costs
#[derive(Clone)]
pub struct BudgetManager {
    daily_limit: Decimal,
    monthly_limit: Decimal,
    spent_today: std::sync::Arc<AtomicU64>,
    spent_month: std::sync::Arc<AtomicU64>,
}

impl BudgetManager {
    pub fn new() -> Self {
        Self {
            daily_limit: Decimal::from_f64_retain(1.0).unwrap_or_default(),
            monthly_limit: Decimal::from_f64_retain(10.0).unwrap_or_default(),
            spent_today: std::sync::Arc::new(AtomicU64::new(0)),
            spent_month: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_limits(daily_limit: f64, monthly_limit: f64) -> Self {
        Self {
            daily_limit: Decimal::from_f64_retain(daily_limit).unwrap_or_default(),
            monthly_limit: Decimal::from_f64_retain(monthly_limit).unwrap_or_default(),
            spent_today: std::sync::Arc::new(AtomicU64::new(0)),
            spent_month: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    /// Check if we can afford this cost
    pub fn can_afford(&self, estimated_cost: Decimal) -> bool {
        // Always allow free
        if estimated_cost == Decimal::ZERO {
            return true;
        }

        let today_spent = Decimal::from(self.spent_today.load(Ordering::Relaxed));
        let month_spent = Decimal::from(self.spent_month.load(Ordering::Relaxed));

        today_spent + estimated_cost <= self.daily_limit
            && month_spent + estimated_cost <= self.monthly_limit
    }

    /// Convenience method to check if we can spend an amount (in dollars)
    pub fn can_spend(&self, amount: f64) -> bool {
        if let Some(cost) = Decimal::from_f64_retain(amount) {
            self.can_afford(cost)
        } else {
            false
        }
    }

    /// Record a spent amount
    pub fn record_spend(&self, amount: f64) {
        let cents = (amount * 100.0) as u64;
        self.spent_today.fetch_add(cents, Ordering::Relaxed);
        self.spent_month.fetch_add(cents, Ordering::Relaxed);
    }
}
