use serde::{Deserialize, Serialize};

/// Hexagon register classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegisterClass {
    IntRegs,
    DoubleRegs,
    PredRegs,
    HvxVR,
    HvxWR,
    HvxQR,
    ModRegs,
    GuestRegs,
    GuestRegs64,
    CtrRegs,
    CtrRegs64,
}

impl RegisterClass {
    /// Parse a register class name string from tablegen.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "IntRegs" => Some(Self::IntRegs),
            "DoubleRegs" => Some(Self::DoubleRegs),
            "PredRegs" => Some(Self::PredRegs),
            "HvxVR" => Some(Self::HvxVR),
            "HvxWR" => Some(Self::HvxWR),
            "HvxQR" => Some(Self::HvxQR),
            "ModRegs" => Some(Self::ModRegs),
            "GuestRegs" => Some(Self::GuestRegs),
            "GuestRegs64" => Some(Self::GuestRegs64),
            "CtrRegs" => Some(Self::CtrRegs),
            "CtrRegs64" => Some(Self::CtrRegs64),
            _ => None,
        }
    }

    /// Number of registers in this class.
    pub fn count(self) -> usize {
        match self {
            Self::IntRegs => 32,
            Self::DoubleRegs => 16,
            Self::PredRegs => 4,
            Self::HvxVR => 32,
            Self::HvxWR => 16,
            Self::HvxQR => 4,
            Self::ModRegs => 2,
            Self::GuestRegs => 32,
            Self::GuestRegs64 => 16,
            Self::CtrRegs => 32,
            Self::CtrRegs64 => 16,
        }
    }

    /// Return the register name prefix, e.g. "r" for IntRegs.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::IntRegs => "r",
            Self::DoubleRegs => "r",
            Self::PredRegs => "p",
            Self::HvxVR => "v",
            Self::HvxWR => "v",
            Self::HvxQR => "q",
            Self::ModRegs => "m",
            Self::GuestRegs => "g",
            Self::GuestRegs64 => "g",
            Self::CtrRegs => "c",
            Self::CtrRegs64 => "c",
        }
    }

    /// Return the concrete register name for a given index.
    pub fn register_name(self, idx: usize) -> String {
        match self {
            Self::IntRegs => format!("r{}", idx),
            Self::DoubleRegs => format!("r{}:{}", idx * 2 + 1, idx * 2),
            Self::PredRegs => format!("p{}", idx),
            Self::HvxVR => format!("v{}", idx),
            Self::HvxWR => format!("v{}:{}", idx * 2 + 1, idx * 2),
            Self::HvxQR => format!("q{}", idx),
            Self::ModRegs => format!("m{}", idx),
            Self::GuestRegs => format!("g{}", idx),
            Self::GuestRegs64 => format!("g{}:{}", idx * 2 + 1, idx * 2),
            Self::CtrRegs => format!("c{}", idx),
            Self::CtrRegs64 => format!("c{}:{}", idx * 2 + 1, idx * 2),
        }
    }

    /// Returns the set of register indices that are safe to use in synthesized tests.
    /// Excludes reserved registers: SP=r29, FP=r30, LR=r31, and r28 (loop counter).
    pub fn safe_indices(self) -> Vec<usize> {
        match self {
            Self::IntRegs => (0..28).collect(), // r0-r27, skip r28 (loop ctr), r29/r30/r31
            Self::DoubleRegs => (0..14).collect(), // r1:0 through r27:26, skip r29:28 and r31:30
            Self::PredRegs => (0..4).collect(),
            Self::HvxVR => (0..32).collect(),
            Self::HvxWR => (0..16).collect(),
            Self::HvxQR => (0..4).collect(),
            Self::ModRegs => (0..2).collect(),
            _ => Vec::new(), // Don't use guest/ctr regs in tests
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_names() {
        assert_eq!(RegisterClass::IntRegs.register_name(0), "r0");
        assert_eq!(RegisterClass::IntRegs.register_name(31), "r31");
        assert_eq!(RegisterClass::DoubleRegs.register_name(0), "r1:0");
        assert_eq!(RegisterClass::DoubleRegs.register_name(1), "r3:2");
        assert_eq!(RegisterClass::PredRegs.register_name(0), "p0");
        assert_eq!(RegisterClass::HvxVR.register_name(5), "v5");
        assert_eq!(RegisterClass::HvxWR.register_name(0), "v1:0");
    }

    #[test]
    fn test_safe_indices_exclude_reserved() {
        let safe = RegisterClass::IntRegs.safe_indices();
        assert!(!safe.contains(&28)); // loop counter
        assert!(!safe.contains(&29)); // SP
        assert!(!safe.contains(&30)); // FP
        assert!(!safe.contains(&31)); // LR
        assert!(safe.contains(&0));
        assert!(safe.contains(&27));
    }

    #[test]
    fn test_from_str() {
        assert_eq!(
            RegisterClass::parse("IntRegs"),
            Some(RegisterClass::IntRegs)
        );
        assert_eq!(RegisterClass::parse("HvxVR"), Some(RegisterClass::HvxVR));
        assert_eq!(RegisterClass::parse("Unknown"), None);
    }
}
