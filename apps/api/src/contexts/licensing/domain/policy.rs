use crate::contexts::licensing::domain::state::LicenseLifecycleState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginSeatDecision {
    AllowExistingSeat,
    AllowNewSeat,
    Deny {
        code: &'static str,
        message: &'static str,
    },
}

pub struct LicensingPolicyEvaluator;

impl LicensingPolicyEvaluator {
    pub fn decide_login(
        state: LicenseLifecycleState,
        has_active_seat: bool,
        active_seat_count: i64,
        max_seats: i32,
    ) -> LoginSeatDecision {
        if has_active_seat {
            return LoginSeatDecision::AllowExistingSeat;
        }

        match state {
            LicenseLifecycleState::Suspended => LoginSeatDecision::Deny {
                code: "ENTITLEMENT_SUSPENDED",
                message: "The issuer refused this entitlement. New seat allocation is blocked.",
            },
            LicenseLifecycleState::Inactive
            | LicenseLifecycleState::Active
            | LicenseLifecycleState::Degraded
            | LicenseLifecycleState::Unknown => {
                if active_seat_count >= i64::from(max_seats) {
                    return LoginSeatDecision::Deny {
                        code: "LICENSE_SEAT_LIMIT_EXCEEDED",
                        message: "No seats available for new users.",
                    };
                }
                LoginSeatDecision::AllowNewSeat
            }
            LicenseLifecycleState::Grace | LicenseLifecycleState::Expired => {
                LoginSeatDecision::Deny {
                    code: "LICENSE_GRACE_ENDED",
                    message: "License grace period ended. New seat allocation is blocked.",
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LicensingPolicyEvaluator, LoginSeatDecision};
    use crate::contexts::licensing::domain::state::LicenseLifecycleState;

    #[test]
    fn expired_denies_new_seat() {
        let decision =
            LicensingPolicyEvaluator::decide_login(LicenseLifecycleState::Expired, false, 0, 5);
        assert!(matches!(
            decision,
            LoginSeatDecision::Deny {
                code: "LICENSE_GRACE_ENDED",
                ..
            }
        ));
    }

    #[test]
    fn active_allows_new_when_capacity_exists() {
        let decision =
            LicensingPolicyEvaluator::decide_login(LicenseLifecycleState::Active, false, 2, 5);
        assert_eq!(decision, LoginSeatDecision::AllowNewSeat);
    }
}
