pub trait SafeDiv: Sized {
    fn safe_div(self, rhs: Self) -> Self;
}

pub trait SafeRem: Sized {
    fn safe_rem(self, rhs: Self) -> Self;
}

pub fn safe_div<T: SafeDiv>(lhs: T, rhs: T) -> T {
    lhs.safe_div(rhs)
}

pub fn safe_rem<T: SafeRem>(lhs: T, rhs: T) -> T {
    lhs.safe_rem(rhs)
}

pub const fn safe_div_f64_to_usize(lhs: f64, rhs: f64) -> usize {
    if rhs == 0.0 {
        0
    } else {
        (lhs / rhs) as usize
    }
}

macro_rules! impl_safe_checked_arithmetic {
    ($($ty:ty),* $(,)?) => {
        $(
            impl SafeDiv for $ty {
                fn safe_div(self, rhs: Self) -> Self {
                    self.checked_div(rhs).unwrap_or_default()
                }
            }

            impl SafeRem for $ty {
                fn safe_rem(self, rhs: Self) -> Self {
                    self.checked_rem(rhs).unwrap_or_default()
                }
            }
        )*
    };
}

impl_safe_checked_arithmetic!(usize, u32, u16, u8, i32, i64, isize, u64);

impl SafeDiv for f32 {
    fn safe_div(self, rhs: Self) -> Self {
        if rhs.abs() <= f32::EPSILON {
            0.0
        } else {
            self / rhs
        }
    }
}

impl SafeDiv for f64 {
    fn safe_div(self, rhs: Self) -> Self {
        if rhs.abs() <= f64::EPSILON {
            0.0
        } else {
            self / rhs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_div, safe_rem};

    #[test]
    fn safe_div_returns_zero_for_zero_integer_divisor() {
        assert_eq!(safe_div(10usize, 0usize), 0);
    }

    #[test]
    fn safe_rem_returns_zero_for_zero_integer_divisor() {
        assert_eq!(safe_rem(10usize, 0usize), 0);
    }

    #[test]
    fn safe_div_returns_zero_for_zero_float_divisor() {
        assert_eq!(safe_div(10.0f32, 0.0f32), 0.0);
    }

    #[test]
    fn safe_div_preserves_nonzero_float_division() {
        assert_eq!(safe_div(10.0f64, 2.0f64), 5.0);
    }
}
