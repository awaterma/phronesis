// Adversarial case 20: a clone whose operand expression, after collapsing
// whitespace, exceeds 240 bytes (exercising D7's digest branch), plus one
// that is comfortably under the cap.

fn long_operand_clone() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let _cloned = data
        .iter()
        .filter(|x| x.is_positive())
        .map(|x| x * 2)
        .filter(|x| x.is_negative().not())
        .cloned()
        .collect::<Vec<i32>>();
}

// Additional function exercising the >240-byte digest branch for `.clone()`.
// The operand of `.clone()` is a long chain of field accesses and method calls.
fn long_operand_dot_clone() {
    let data = SomeData;
    let _cloned = data
        .get_first_compound_component_name()
        .get_second_compound_component_name()
        .get_third_compound_component_name()
        .get_fourth_compound_component_name()
        .get_fifth_compound_component_name()
        .get_sixth_compound_component_name()
        .get_seventh_compound_component_name()
        .get_eighth_compound_component_name()
        .get_ninth_compound_component_name()
        .get_tenth_compound_component_name()
        .get_eleventh_compound_component_name()
        .get_twelfth_compound_component_name()
        .clone();
}

fn short_operand_clone() {
    let small = 42i32;
    let _cloned = small.clone();
}

struct SomeData;
impl SomeData {
    fn get_first_compound_component_name(&self) -> Self { Self }
    fn get_second_compound_component_name(&self) -> Self { Self }
    fn get_third_compound_component_name(&self) -> Self { Self }
    fn get_fourth_compound_component_name(&self) -> Self { Self }
    fn get_fifth_compound_component_name(&self) -> Self { Self }
    fn get_sixth_compound_component_name(&self) -> Self { Self }
    fn get_seventh_compound_component_name(&self) -> Self { Self }
    fn get_eighth_compound_component_name(&self) -> Self { Self }
    fn get_ninth_compound_component_name(&self) -> Self { Self }
    fn get_tenth_compound_component_name(&self) -> Self { Self }
    fn get_eleventh_compound_component_name(&self) -> Self { Self }
    fn get_twelfth_compound_component_name(&self) -> Self { Self }
}
