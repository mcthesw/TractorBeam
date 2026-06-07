pub const PRODUCT_NAME: &str = "Basement Bridge";

pub fn runtime_name() -> &'static str {
    "bridge-client"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_runtime_name() {
        assert_eq!(runtime_name(), "bridge-client");
    }
}
