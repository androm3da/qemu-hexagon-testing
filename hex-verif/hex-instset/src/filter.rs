use hex_dump::types::InstructionDef;

/// Direction of an operand for register class filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperandDir {
    /// Match any operand (input or output).
    Any,
    /// Match only input operands.
    Input,
    /// Match only output operands.
    Output,
}

/// A composable filter for querying instructions.
#[derive(Debug, Clone)]
pub enum Filter {
    /// Match instructions of a specific IType.
    ByType(String),
    /// Match instructions whose assembly syntax contains the given substring.
    BySyntaxContains(String),
    /// Match instructions whose assembly syntax does NOT contain the given substring.
    BySyntaxNotContains(String),
    /// Match instructions whose name contains the given substring.
    ByNameContains(String),
    /// Match by a boolean attribute.
    ByAttribute(AttributeFilter),
    /// Match instructions that require a specific feature.
    ByRequires(String),
    /// Match instructions that have an operand of the given register class.
    HasRegClassOperand {
        class: String,
        direction: OperandDir,
    },
    /// Match instructions that have at least one immediate operand.
    HasImmediateOperand,
    /// Logical AND of multiple filters.
    And(Vec<Filter>),
    /// Logical OR of multiple filters.
    Or(Vec<Filter>),
    /// Logical NOT of a filter.
    Not(Box<Filter>),
}

/// Boolean attribute filters.
#[derive(Debug, Clone)]
pub enum AttributeFilter {
    IsSolo(bool),
    IsSoloAX(bool),
    IsPredicated(bool),
    IsPredicatedNew(bool),
    HasNewValue(bool),
    IsNvStore(bool),
    IsCvi(bool),
    IsHvxAlu(bool),
    MayLoad(bool),
    MayStore(bool),
    IsCommutable(bool),
    IsPredicable(bool),
    IsExtendable(bool),
    HasSideEffects(bool),
    IsCall(bool),
    IsReturn(bool),
    IsFp(bool),
}

impl Filter {
    /// Test whether an instruction matches this filter.
    pub fn matches(&self, insn: &InstructionDef) -> bool {
        match self {
            Filter::ByType(t) => insn.itype == *t,
            Filter::BySyntaxContains(s) => {
                insn.asm_syntax.to_lowercase().contains(&s.to_lowercase())
            }
            Filter::BySyntaxNotContains(s) => {
                !insn.asm_syntax.to_lowercase().contains(&s.to_lowercase())
            }
            Filter::ByNameContains(s) => insn.name.to_lowercase().contains(&s.to_lowercase()),
            Filter::ByAttribute(attr) => match_attribute(insn, attr),
            Filter::ByRequires(feature) => insn.requires.iter().any(|r| r.contains(feature)),
            Filter::HasRegClassOperand { class, direction } => {
                match_reg_class_operand(insn, class, direction)
            }
            Filter::HasImmediateOperand => {
                insn.ins.iter().any(|op| op.is_immediate)
                    || insn.outs.iter().any(|op| op.is_immediate)
            }
            Filter::And(filters) => filters.iter().all(|f| f.matches(insn)),
            Filter::Or(filters) => filters.iter().any(|f| f.matches(insn)),
            Filter::Not(f) => !f.matches(insn),
        }
    }
}

fn match_attribute(insn: &InstructionDef, attr: &AttributeFilter) -> bool {
    match attr {
        AttributeFilter::IsSolo(v) => insn.is_solo == *v,
        AttributeFilter::IsSoloAX(v) => insn.is_solo_ax == *v,
        AttributeFilter::IsPredicated(v) => insn.is_predicated == *v,
        AttributeFilter::IsPredicatedNew(v) => insn.is_predicated_new == *v,
        AttributeFilter::HasNewValue(v) => insn.has_new_value == *v,
        AttributeFilter::IsNvStore(v) => insn.is_nv_store == *v,
        AttributeFilter::IsCvi(v) => insn.is_cvi == *v,
        AttributeFilter::IsHvxAlu(v) => insn.is_hvx_alu == *v,
        AttributeFilter::MayLoad(v) => insn.may_load == *v,
        AttributeFilter::MayStore(v) => insn.may_store == *v,
        AttributeFilter::IsCommutable(v) => insn.is_commutable == *v,
        AttributeFilter::IsPredicable(v) => insn.is_predicable == *v,
        AttributeFilter::IsExtendable(v) => insn.is_extendable == *v,
        AttributeFilter::HasSideEffects(v) => insn.has_side_effects == *v,
        AttributeFilter::IsCall(v) => insn.is_call == *v,
        AttributeFilter::IsReturn(v) => insn.is_return == *v,
        AttributeFilter::IsFp(v) => insn.is_fp == *v,
    }
}

fn match_reg_class_operand(insn: &InstructionDef, class: &str, direction: &OperandDir) -> bool {
    let check_outs = matches!(direction, OperandDir::Any | OperandDir::Output);
    let check_ins = matches!(direction, OperandDir::Any | OperandDir::Input);

    if check_outs
        && insn
            .outs
            .iter()
            .any(|op| op.reg_class.as_deref() == Some(class))
    {
        return true;
    }
    if check_ins
        && insn
            .ins
            .iter()
            .any(|op| op.reg_class.as_deref() == Some(class))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_dump::types::InstructionDef;

    fn make_alu_insn() -> InstructionDef {
        let mut insn = InstructionDef::new("A2_add".to_string());
        insn.itype = "TypeALU32_3op".to_string();
        insn.asm_syntax = "$Rd32 = add($Rs32,$Rt32)".to_string();
        insn.has_new_value = true;
        insn.is_commutable = true;
        insn
    }

    #[test]
    fn test_filter_by_type() {
        let insn = make_alu_insn();
        assert!(Filter::ByType("TypeALU32_3op".to_string()).matches(&insn));
        assert!(!Filter::ByType("TypeLD".to_string()).matches(&insn));
    }

    #[test]
    fn test_filter_by_syntax() {
        let insn = make_alu_insn();
        assert!(Filter::BySyntaxContains("add".to_string()).matches(&insn));
        assert!(!Filter::BySyntaxContains("sub".to_string()).matches(&insn));
    }

    #[test]
    fn test_filter_and() {
        let insn = make_alu_insn();
        let f = Filter::And(vec![
            Filter::ByType("TypeALU32_3op".to_string()),
            Filter::ByAttribute(AttributeFilter::HasNewValue(true)),
        ]);
        assert!(f.matches(&insn));
    }

    #[test]
    fn test_filter_not() {
        let insn = make_alu_insn();
        let f = Filter::Not(Box::new(Filter::ByType("TypeLD".to_string())));
        assert!(f.matches(&insn));
    }
}
