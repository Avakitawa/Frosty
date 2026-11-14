#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnowfallState {
    enabled: bool,
}

impl SnowfallState {
    pub const fn new() -> Self {
        Self { enabled: true }
    }

    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    pub fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }
}

impl Default for SnowfallState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::SnowfallState;

    #[test]
    fn snowfall_starts_enabled_and_can_be_toggled() {
        let mut state = SnowfallState::default();

        assert!(state.is_enabled());
        assert!(!state.toggle());
        assert!(!state.is_enabled());
        assert!(state.toggle());
        assert!(state.is_enabled());
    }
}
