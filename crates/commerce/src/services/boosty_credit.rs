//! D-10: the provider collector owns authentication and interpretation;
//! this service only forwards already verified, net-RUB charge evidence.
//! There is deliberately no public or browser callable funding endpoint.
use crate::domain::boosty_credit::{
    BoostyCreditError, BoostyCreditReceipt, BoostyCreditRepository, VerifiedSubscriptionCharge,
};

pub struct BoostyWalletFunding<R: BoostyCreditRepository> {
    repository: R,
}

impl<R: BoostyCreditRepository> BoostyWalletFunding<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub async fn apply_verified_charge(
        &self,
        charge: &VerifiedSubscriptionCharge,
    ) -> Result<BoostyCreditReceipt, BoostyCreditError> {
        self.repository.credit_once(charge).await
    }
}
