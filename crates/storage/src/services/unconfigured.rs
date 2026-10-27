//! Safe default until the real G adapter can verify an attached private S3
//! object and issue a 90-second signed GET with a scoped content key.
use async_trait::async_trait;
use std::time::Duration;
use tor_contracts::paid_delivery::{
    DeliveryError, EncryptedObjectRef, GrantedContent, PrivateObjectDeliveryPort,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct UnconfiguredPrivateDelivery;

#[async_trait]
impl PrivateObjectDeliveryPort for UnconfiguredPrivateDelivery {
    async fn grant_verified_object(
        &self,
        _object: &EncryptedObjectRef,
        _ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        Err(DeliveryError::Unavailable)
    }
}
