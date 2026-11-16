//! A source supplied by the web composition must match B's exact edition hash.
//! The caller still locks and authorizes the edition in its own E transaction.
use sha2::{Digest, Sha256};

pub(crate) fn resolve(
    stored: Option<String>,
    hash: Option<Vec<u8>>,
    verified: Option<&str>,
) -> Option<String> {
    if let Some(stored) = stored {
        return Some(stored);
    }
    let (Some(hash), Some(source)) = (hash, verified) else {
        return None;
    };
    let actual: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    (hash == actual).then(|| source.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_requires_the_exact_immutable_digest() {
        let source = "Точная редакция😀";
        let hash = Sha256::digest(source.as_bytes()).to_vec();
        assert_eq!(
            resolve(None, Some(hash.clone()), Some(source)),
            Some(source.to_owned())
        );
        assert_eq!(resolve(None, Some(hash), Some("Подмена")), None);
        assert_eq!(resolve(None, None, Some(source)), None);
        assert_eq!(
            resolve(Some(source.to_owned()), None, None),
            Some(source.to_owned())
        );
    }
}
