pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divides_positive_values() {
        assert_eq!(safe_divide(8, 2), Ok(4));
    }

    #[test]
    fn divides_negative_values() {
        assert_eq!(safe_divide(-8, 2), Ok(-4));
    }

    #[test]
    fn rejects_zero_denominator() {
        assert_eq!(safe_divide(8, 0), Err("division by zero"));
    }
}