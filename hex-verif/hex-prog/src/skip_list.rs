/// Default terms that should be excluded from synthesized instructions.
/// Instructions whose name or assembly syntax (case-insensitive) contain any of these
/// terms are skipped during synthesis.
pub const DEFAULT_SKIP_TERMS: &[&str] = &[
    "mem",
    "swi",
    "trap",
    "scatter",
    "gather",
    ":raw",
    ".new",
    "cmpyiw",
    "cmpyrw",
    "jump",
    "dfmake",
    "sfmake",
    "convert",
    "decbin",
    "call",
    "callr",
    "jumpr",
    "nmi",
    "stop",
    "rte",
    "wait",
    "icinv",
    "tlbw",
    "tlbp",
    "tlbinvasid",
    "k0lock",
    "k0unlock",
    "tlblock",
    "tlbunlock",
    "vhist",
    "vwhist",
    "dealloc",
    "loop",
    "dcclean",
    "dcinv",
    "l2lock",
    "l2unlock",
    "l2fetch",
    "release",
    "dmlink",
    "start",
    "setprio",
    "diag",
    "dczero",
    "resume",
    "sfmax",
    "sfmin",
    "dfmin",
    "dfmax",
    "vshuff",
    "vdeal",
    "vcombine",
    "allocframe",
    "pause",
    "r29",
    "r30",
    "r31",
];

/// Check if an instruction name or syntax matches any term in the skip list.
pub fn matches_skip_list(name: &str, asm_syntax: &str, skip_terms: &[&str]) -> bool {
    let name_lower = name.to_lowercase();
    let asm_lower = asm_syntax.to_lowercase();
    skip_terms.iter().any(|term| {
        let term_lower = term.to_lowercase();
        name_lower.contains(&term_lower) || asm_lower.contains(&term_lower)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_skip_list_name() {
        assert!(matches_skip_list(
            "S2_allocframe",
            "$Rd = allocframe()",
            DEFAULT_SKIP_TERMS
        ));
    }

    #[test]
    fn test_matches_skip_list_syntax() {
        assert!(matches_skip_list("A2_foo", "call $Ii", DEFAULT_SKIP_TERMS));
    }

    #[test]
    fn test_no_match() {
        assert!(!matches_skip_list(
            "A2_add",
            "$Rd32 = add($Rs32,$Rt32)",
            DEFAULT_SKIP_TERMS
        ));
    }

    #[test]
    fn test_r29_match() {
        assert!(matches_skip_list(
            "A2_foo",
            "$Rd32 = add(r29,$Rt32)",
            DEFAULT_SKIP_TERMS
        ));
    }
}
