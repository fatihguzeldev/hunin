//! Nominal identities at acceptance boundaries. The observation schema owns
//! wire validation; these types neither normalize strings nor require UUIDs.
use serde::{Deserialize, Serialize};
use std::{borrow::Borrow, fmt};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ObservationId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ResultId(String);

macro_rules! identity {
    ($name:ident) => {
        impl $name {
            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
identity!(ObservationId);
identity!(ResultId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_codecs_preserve_opaque_strings_without_uuid_or_normalization_rules() {
        for value in ["raw:α/quoted\"", "观测:1", "mixed-case-ID", ""] {
            let observation = ObservationId::from(value);
            let result = ResultId::from(value);
            let encoded = serde_json::to_string(value).unwrap();
            assert_eq!(serde_json::to_string(&observation).unwrap(), encoded);
            assert_eq!(serde_json::to_string(&result).unwrap(), encoded);
            assert_eq!(
                serde_json::from_str::<ObservationId>(&encoded).unwrap(),
                observation
            );
            assert_eq!(serde_json::from_str::<ResultId>(&encoded).unwrap(), result);
            assert_eq!(observation.to_string(), value);
            assert_eq!(result.to_string(), value);
        }
    }
}
