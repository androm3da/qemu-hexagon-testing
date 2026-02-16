use hex_instset::database::slot_mask_for_itype;

/// A VLIW execution slot (0-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Slot0 = 0,
    Slot1 = 1,
    Slot2 = 2,
    Slot3 = 3,
}

impl Slot {
    pub fn all() -> [Slot; 4] {
        [Slot::Slot0, Slot::Slot1, Slot::Slot2, Slot::Slot3]
    }

    pub fn mask(self) -> u8 {
        1 << (self as u8)
    }
}

/// Attempt to assign slots to a list of instructions (given as IType strings).
/// Returns Some(vec of slot assignments) on success, None if no valid assignment exists.
/// Uses a greedy algorithm: assigns most restrictive instructions first.
pub fn assign_slots(itypes: &[&str]) -> Option<Vec<Slot>> {
    let n = itypes.len();
    if n == 0 || n > 4 {
        return None;
    }

    // Build (index, slot_mask) pairs, sorted by restrictiveness (fewest bits first)
    let mut entries: Vec<(usize, u8)> = itypes
        .iter()
        .enumerate()
        .map(|(i, itype)| (i, slot_mask_for_itype(itype)))
        .collect();
    entries.sort_by_key(|&(_, mask)| mask.count_ones());

    let mut used: u8 = 0;
    let mut assignments = vec![Slot::Slot0; n];

    for &(idx, mask) in &entries {
        let available = mask & !used;
        if available == 0 {
            return None;
        }
        // Pick the lowest available slot
        let slot_bit = available & available.wrapping_neg(); // isolate lowest set bit
        let slot_num = slot_bit.trailing_zeros() as usize;
        assignments[idx] = match slot_num {
            0 => Slot::Slot0,
            1 => Slot::Slot1,
            2 => Slot::Slot2,
            3 => Slot::Slot3,
            _ => return None,
        };
        used |= slot_bit;
    }

    Some(assignments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assign_single_alu() {
        let result = assign_slots(&["TypeALU32_3op"]);
        assert!(result.is_some());
    }

    #[test]
    fn test_assign_store_and_load() {
        let result = assign_slots(&["TypeST", "TypeLD"]);
        assert!(result.is_some());
        let slots = result.unwrap();
        assert_eq!(slots[0], Slot::Slot0); // ST goes to slot 0
        assert_eq!(slots[1], Slot::Slot1); // LD goes to slot 1
    }

    #[test]
    fn test_assign_two_stores_fails() {
        let result = assign_slots(&["TypeST", "TypeST"]);
        assert!(result.is_none()); // Both need slot 0
    }

    #[test]
    fn test_assign_full_packet() {
        let result = assign_slots(&["TypeST", "TypeLD", "TypeALU64", "TypeALU32_3op"]);
        assert!(result.is_some());
        let slots = result.unwrap();
        assert_eq!(slots[0], Slot::Slot0); // ST
        assert_eq!(slots[1], Slot::Slot1); // LD
        assert!(slots[2] == Slot::Slot2 || slots[2] == Slot::Slot3); // ALU64
    }

    #[test]
    fn test_assign_empty() {
        assert!(assign_slots(&[]).is_none());
    }

    #[test]
    fn test_assign_too_many() {
        let result = assign_slots(&[
            "TypeALU32_3op",
            "TypeALU32_3op",
            "TypeALU32_3op",
            "TypeALU32_3op",
            "TypeALU32_3op",
        ]);
        assert!(result.is_none());
    }
}
