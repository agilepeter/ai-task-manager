//! AI Task Manager core: everything that reads usage, spend and the local AI
//! setup, with no UI and no Tauri. The tray app is one consumer; a headless
//! per-seat agent is meant to be another.

pub mod alerts;
pub mod httpapi;
pub mod i18n;
pub mod inventory;
pub mod pricing;
pub mod providers;
pub mod rt;
pub mod spend;

/// The provider family of a card id: "claude@ab12cd34" → "claude".
pub fn family_of(id: &str) -> String {
    id.split('@').next().unwrap_or(id).to_string()
}

/// Cards whose ids are minted per saved key rather than per provider.
pub fn is_managed_key_card(id: &str) -> bool {
    matches!(family_of(id).as_str(), "onenewapi" | "sub2api")
}

/// A card is off when it is disabled by id, or when it is a managed-key card
/// and its whole family is disabled.
pub fn card_is_disabled(id: &str, disabled: &[String]) -> bool {
    if disabled.iter().any(|d| d == id) {
        return true;
    }
    is_managed_key_card(id) && disabled.iter().any(|d| d == &family_of(id))
}
