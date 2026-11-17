//! DROP-07b: bounded, strictly read-only listing of private FREE namespaces.
//! Listing is not proof that an object is orphaned or safe to delete. S3
//! errors, malformed pagination and provider clock uncertainty fail closed.
use aws_sdk_s3::Client;

use crate::domain::provider::{valid_private_bucket, StorageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreeListNamespace {
    Single,
    Pack,
}

impl FreeListNamespace {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Single => "private/free-chapters/",
            Self::Pack => "private/free-packs/",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedFreeObject {
    pub object_key: String,
    pub size_bytes: u64,
    pub modified_at_unix: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeListPage {
    pub listed: Vec<ListedFreeObject>,
    pub next_token: Option<String>,
}

pub struct S3FreeInventoryLister {
    client: Client,
}

impl S3FreeInventoryLister {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// An opaque S3 pagination token is scoped to caller-selected bucket and
    /// namespace. Never accept one supplied by a browser, user or public API.
    pub async fn list_page(
        &self,
        bucket: &str,
        namespace: FreeListNamespace,
        continuation: Option<&str>,
        limit: u32,
    ) -> Result<FreeListPage, StorageError> {
        if !valid_private_bucket(bucket)
            || !(1..=128).contains(&limit)
            || continuation.is_some_and(|token| {
                token.is_empty() || token.len() > 1024 || token.chars().any(char::is_control)
            })
        {
            return Err(StorageError::Invalid);
        }
        let mut request = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(namespace.prefix())
            .max_keys(limit as i32);
        if let Some(token) = continuation {
            request = request.continuation_token(token);
        }
        let response = request.send().await.map_err(|_| StorageError::Unavailable)?;
        let contents = response.contents();
        if contents.len() > limit as usize {
            return Err(StorageError::Corrupt);
        }
        let mut listed = Vec::with_capacity(contents.len());
        let mut previous: Option<&str> = None;
        for object in contents {
            let key = object.key().ok_or(StorageError::Corrupt)?;
            if !key.starts_with(namespace.prefix())
                || key.len() > 260
                || key.contains("//")
                || key
                    .split('/')
                    .any(|segment| segment == "." || segment == "..")
                || key
                    .bytes()
                    .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.')))
                || previous.is_some_and(|last| key <= last)
            {
                return Err(StorageError::Corrupt);
            }
            let size = object.size().ok_or(StorageError::Corrupt)?;
            if size < 0 || size > 16_777_216 {
                return Err(StorageError::Corrupt);
            }
            listed.push(ListedFreeObject {
                object_key: key.to_owned(),
                size_bytes: size as u64,
                modified_at_unix: object.last_modified().map(|date| date.secs()),
            });
            previous = Some(key);
        }
        let has_more = response.is_truncated().unwrap_or(false);
        let next_token = if has_more {
            let token = response
                .next_continuation_token()
                .ok_or(StorageError::Unavailable)?;
            if token.is_empty() || token.len() > 1024 || Some(token) == continuation {
                return Err(StorageError::Unavailable);
            }
            Some(token.to_owned())
        } else {
            None
        };
        Ok(FreeListPage { listed, next_token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fixed_namespaces_cannot_escape_private_free_storage() {
        assert_eq!(FreeListNamespace::Single.prefix(), "private/free-chapters/");
        assert_eq!(FreeListNamespace::Pack.prefix(), "private/free-packs/");
    }
}
