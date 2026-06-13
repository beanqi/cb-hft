use core::fmt;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(pub u16);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub i64);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(pub i64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetId(String);

impl AssetId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecimalParseError {
    Empty,
    InvalidByte(u8),
    TooManyFractionalDigits,
    NegativeNotSupported,
    InvalidScale,
    Overflow,
}

impl fmt::Display for DecimalParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for DecimalParseError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductValidationError {
    PriceNotOnTick,
    QtyBelowMinimum,
    QtyNotOnStep,
    NotionalBelowMinimum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProductSpec {
    pub symbol_id: SymbolId,
    pub coinbase_product: &'static str,
    pub price_scale: i64,
    pub qty_scale: i64,
    pub min_qty: Qty,
    pub min_notional: i64,
    pub price_tick: Price,
    pub qty_step: Qty,
}

impl Price {
    #[inline]
    pub fn parse_scaled(input: &[u8], scale: i64) -> Result<Self, DecimalParseError> {
        parse_decimal_scaled(input, scale).map(Self)
    }
}

impl Qty {
    #[inline]
    pub fn parse_scaled(input: &[u8], scale: i64) -> Result<Self, DecimalParseError> {
        parse_decimal_scaled(input, scale).map(Self)
    }
}

impl ProductSpec {
    pub fn validate_order(&self, price: Price, qty: Qty) -> Result<(), ProductValidationError> {
        if self.price_tick.0 > 0 && price.0 % self.price_tick.0 != 0 {
            return Err(ProductValidationError::PriceNotOnTick);
        }
        if qty.0 < self.min_qty.0 {
            return Err(ProductValidationError::QtyBelowMinimum);
        }
        if self.qty_step.0 > 0 && qty.0 % self.qty_step.0 != 0 {
            return Err(ProductValidationError::QtyNotOnStep);
        }
        let notional = (price.0 as i128) * (qty.0 as i128);
        if notional < self.min_notional as i128 {
            return Err(ProductValidationError::NotionalBelowMinimum);
        }
        Ok(())
    }
}

fn parse_decimal_scaled(input: &[u8], scale: i64) -> Result<i64, DecimalParseError> {
    if input.is_empty() {
        return Err(DecimalParseError::Empty);
    }
    if scale <= 0 {
        return Err(DecimalParseError::InvalidScale);
    }
    if input[0] == b'-' {
        return Err(DecimalParseError::NegativeNotSupported);
    }

    let mut int_part: i128 = 0;
    let mut frac_part: i128 = 0;
    let mut frac_scale: i64 = 1;
    let mut seen_dot = false;
    let mut seen_digit = false;

    for &b in input {
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                let digit = (b - b'0') as i128;
                if seen_dot {
                    frac_scale = frac_scale
                        .checked_mul(10)
                        .ok_or(DecimalParseError::Overflow)?;
                    if frac_scale > scale || scale % frac_scale != 0 {
                        return Err(DecimalParseError::TooManyFractionalDigits);
                    }
                    frac_part = frac_part
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(digit))
                        .ok_or(DecimalParseError::Overflow)?;
                } else {
                    int_part = int_part
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(digit))
                        .ok_or(DecimalParseError::Overflow)?;
                }
            }
            b'.' if !seen_dot => seen_dot = true,
            other => return Err(DecimalParseError::InvalidByte(other)),
        }
    }

    if !seen_digit {
        return Err(DecimalParseError::Empty);
    }

    let scaled = int_part
        .checked_mul(scale as i128)
        .and_then(|v| v.checked_add(frac_part * (scale / frac_scale) as i128))
        .ok_or(DecimalParseError::Overflow)?;

    i64::try_from(scaled).map_err(|_| DecimalParseError::Overflow)
}
