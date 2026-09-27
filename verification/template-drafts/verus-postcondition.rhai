// DRAFT template — (rust, verus, postcondition), seam example only.
//
// Renders a standalone Verus-native module proving the safe_divide
// postconditions (SPEC-verification-artifact-generation.md, C1 subject).
// It proves the seam end to end; it is NOT a general postcondition
// template, and it is a draft: `phr-mcp verify` reads it only with
// `--allow-drafts`, and results from it never count as verified evidence.
//
// Render contract: `property` (read-only map: id, subject, kind, condition,
// guarantee, depends_on — every value escaped by the host) and `facts`
// (frozen, sorted `property_depends_on` facts) are the whole scope. The
// script returns exactly one string. Property values may appear only inside
// comments or string literals: the host validator refuses the body
// otherwise, which is why the harness names its function `divide` rather
// than interpolating the subject into code, and why its parameters are `n`
// and `d`: a condition such as "denominator == 0" must not also occur as
// live code.

if property.subject != "safe_divide" {
    throw `verus-postcondition draft only models the safe_divide subject, not ${property.subject}`;
}

let regions = "";
for fact in facts {
    // A double-quoted string: backtick strings do not process `\n`.
    regions += "// depends_on: " + fact.args[1] + "\n";
}

`// property: ${property.id} (${property.kind})
// condition: ${property.condition}
// guarantee: ${property.guarantee}
${regions}use vstd::prelude::*;

verus! {

pub enum DivResult {
    Ok(u32),
    Err,
}

pub open spec fn claim_holds(n: u32, d: u32, res: DivResult) -> bool {
    &&& (d == 0 ==> matches!(res, DivResult::Err))
    &&& (d != 0 ==> res == DivResult::Ok(n / d))
}

pub fn divide(n: u32, d: u32) -> (res: DivResult)
    ensures
        claim_holds(n, d, res),
{
    if d == 0 {
        DivResult::Err
    } else {
        DivResult::Ok(n / d)
    }
}

fn main() {
}

} // verus!
`
