use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A paying customer, normalized from a billing provider. Phase 1 keeps this as a model anchor —
/// revenue and events carry the `customer_id` string directly, so margin rollups don't yet require a
/// populated customer table; Stripe/Polar sync (Phase 2) upserts these for names/emails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    #[serde(default = "crate::new_id")]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    /// Source billing system, e.g. `stripe` | `polar` | `manual`.
    #[serde(default = "default_source")]
    pub source: String,
    /// The provider's own customer id, e.g. Stripe `cus_…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

/// A billable product/SKU. Used to attribute cost (and, when products map 1:1 to SKUs, revenue) to a
/// feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingProduct {
    #[serde(default = "crate::new_id")]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    /// Source billing system, e.g. `stripe` | `polar` | `manual`.
    #[serde(default = "default_source")]
    pub source: String,
    /// The provider's own product id, e.g. Stripe `prod_…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub name: String,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

fn default_source() -> String {
    "manual".to_string()
}
