#[macro_export]
macro_rules! string_id {
    ($name:ident) => {
        #[derive(Default, Hash, Eq, PartialEq, Clone, Serialize, Deserialize, Debug)]
        pub struct $name(String);

        impl std::ops::Deref for $name {
            type Target = String;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl $name {
            pub fn cloned(&self) -> String {
                self.0.clone()
            }
        }
    };
}
