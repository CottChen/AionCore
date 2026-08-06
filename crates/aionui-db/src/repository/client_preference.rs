use crate::error::DbError;
use crate::models::ClientPreference;

/// Client preference data access abstraction.
///
/// Provides CRUD operations on the generic key-value `client_preferences` table.
#[async_trait::async_trait]
pub trait IClientPreferenceRepository: Send + Sync {
    /// Returns all client preferences.
    async fn get_all(&self, user_id: &str) -> Result<Vec<ClientPreference>, DbError>;

    /// Returns preferences for the given keys only.
    /// Keys that don't exist are simply omitted from the result.
    async fn get_by_keys(&self, user_id: &str, keys: &[&str]) -> Result<Vec<ClientPreference>, DbError>;

    /// Returns all client preferences overridden by a specific user.
    async fn get_all_for_user(&self, user_id: &str) -> Result<Vec<ClientPreference>, DbError> {
        self.get_all(user_id).await
    }

    /// Returns user-scoped preferences for the given keys only.
    async fn get_by_keys_for_user(&self, user_id: &str, keys: &[&str]) -> Result<Vec<ClientPreference>, DbError> {
        self.get_by_keys(user_id, keys).await
    }

    /// Inserts or updates a batch of key-value pairs.
    async fn upsert_batch(&self, user_id: &str, entries: &[(&str, &str)]) -> Result<(), DbError>;

    /// Inserts or updates a batch of key-value pairs for a specific user.
    async fn upsert_batch_for_user(&self, user_id: &str, entries: &[(&str, &str)]) -> Result<(), DbError> {
        self.upsert_batch(user_id, entries).await
    }

    /// Deletes the given keys.
    async fn delete_keys(&self, user_id: &str, keys: &[&str]) -> Result<(), DbError>;

    /// Deletes the given user-scoped keys.
    async fn delete_keys_for_user(&self, user_id: &str, keys: &[&str]) -> Result<(), DbError> {
        self.delete_keys(user_id, keys).await
    }
}
