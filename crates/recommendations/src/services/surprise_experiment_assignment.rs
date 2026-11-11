//! Independent assignment domains for separate runs; persistence owns the arm
//! after enrollment, even if the deployment rotates its secret later.
use super::surprise_schedule::{experiment_arm, ExperimentArm};
use sha2::{Digest, Sha256};

pub fn for_run(key: &[u8; 32], subject: &[u8; 32], run_id: i64) -> ExperimentArm {
    let mut digest = Sha256::new();
    digest.update(b"TOR-surprise-experiment-run-v1");
    digest.update(run_id.to_be_bytes());
    digest.update(subject);
    experiment_arm(key, &digest.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_run_is_stable_but_a_new_run_does_not_reuse_the_partition() {
        let key = [0x71; 32];
        let subjects: Vec<[u8; 32]> = (0_i64..1000)
            .map(|id| {
                let mut subject = [0_u8; 32];
                subject[..8].copy_from_slice(&id.to_be_bytes());
                subject
            })
            .collect();
        let first: Vec<_> = subjects.iter().map(|s| for_run(&key, s, 1)).collect();
        let repeat: Vec<_> = subjects.iter().map(|s| for_run(&key, s, 1)).collect();
        let second: Vec<_> = subjects.iter().map(|s| for_run(&key, s, 2)).collect();
        assert_eq!(first, repeat);
        assert_ne!(first, second);
    }
}
