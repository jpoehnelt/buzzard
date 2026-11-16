//! Scaling of raw values to engineering units, as channel properties
//! describe it.
//!
//! The format description shows only the linear scale. The other kinds
//! (polynomial, thermocouple, RTD, strain, table, thermistor, add,
//! subtract) are recognized by name and reported as unsupported; nothing is
//! guessed.

use crate::types::{Property, PropertyValue};

/// What has to be done to a channel's raw values to get engineering units.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Scaling {
    /// Nothing: the channel has no scaling properties, or says that its
    /// values are already scaled.
    None,
    /// `raw * slope + intercept`. A chain of linear scales is folded into
    /// one.
    Linear {
        /// The factor.
        slope: f64,
        /// The offset.
        intercept: f64,
        /// For a channel in a driver's raw buffers: the scaler whose raw
        /// values the chain starts from.
        scaler: Option<u32>,
    },
    /// The chain contains a scale of this type, which is not supported.
    Unsupported(String),
}

fn find<'a>(props: &'a [Property], name: &str) -> Option<&'a PropertyValue> {
    props.iter().find(|p| p.name == name).map(|p| &p.value)
}

impl Scaling {
    /// Reads the `NI_Scaling_Status`, `NI_Number_Of_Scales` and
    /// `NI_Scale[n]_...` properties.
    ///
    /// The last scale is the channel's output. Each scale names its input
    /// by number; a number without a scale type is the raw data (for a
    /// driver channel, the scaler with that id).
    pub(crate) fn of(props: &[Property], has_scalers: bool) -> Self {
        let Some(count) = find(props, "NI_Number_Of_Scales").and_then(PropertyValue::as_u64) else {
            return Self::None;
        };
        if find(props, "NI_Scaling_Status").and_then(PropertyValue::as_str) == Some("scaled")
            || count == 0
        {
            return Self::None;
        }
        let (mut slope, mut intercept) = (1.0, 0.0);
        let mut at = count - 1;
        // A chain cannot be longer than the number of scales.
        for _ in 0..count.min(1 << 16) {
            let Some(kind) =
                find(props, &format!("NI_Scale[{at}]_Scale_Type")).and_then(PropertyValue::as_str)
            else {
                break;
            };
            if kind != "Linear" {
                return Self::Unsupported(kind.to_owned());
            }
            let number = |name: &str| {
                find(props, &format!("NI_Scale[{at}]_Linear_{name}"))
                    .and_then(PropertyValue::as_f64)
            };
            let (s, i) = (
                number("Slope").unwrap_or(1.0),
                number("Y_Intercept").unwrap_or(0.0),
            );
            // The scales so far are applied after this one.
            intercept += slope * i;
            slope *= s;
            match find(props, &format!("NI_Scale[{at}]_Linear_Input_Source"))
                .and_then(PropertyValue::as_u64)
            {
                Some(source) if source < at => at = source,
                _ => {
                    at = u64::MAX;
                    break;
                }
            }
        }
        let scaler = if has_scalers {
            u32::try_from(at).ok()
        } else {
            None
        };
        if has_scalers && scaler.is_none() {
            // The chain does not end at a scaler: nothing to start from.
            return Self::Unsupported("a chain without a raw input".to_owned());
        }
        Self::Linear {
            slope,
            intercept,
            scaler,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DataType;

    fn prop(name: &str, value: PropertyValue) -> Property {
        Property {
            name: name.to_owned(),
            dtype: DataType::Void,
            value,
        }
    }

    #[test]
    fn linear_chain_folds() {
        let props = vec![
            prop(
                "NI_Scaling_Status",
                PropertyValue::String("unscaled".into()),
            ),
            prop("NI_Number_Of_Scales", PropertyValue::U32(3)),
            prop(
                "NI_Scale[1]_Scale_Type",
                PropertyValue::String("Linear".into()),
            ),
            prop("NI_Scale[1]_Linear_Slope", PropertyValue::F64(2.0)),
            prop("NI_Scale[1]_Linear_Y_Intercept", PropertyValue::F64(1.0)),
            prop("NI_Scale[1]_Linear_Input_Source", PropertyValue::U32(0)),
            prop(
                "NI_Scale[2]_Scale_Type",
                PropertyValue::String("Linear".into()),
            ),
            prop("NI_Scale[2]_Linear_Slope", PropertyValue::F64(10.0)),
            prop("NI_Scale[2]_Linear_Y_Intercept", PropertyValue::F64(5.0)),
            prop("NI_Scale[2]_Linear_Input_Source", PropertyValue::U32(1)),
        ];
        // (x * 2 + 1) * 10 + 5
        assert_eq!(
            Scaling::of(&props, true),
            Scaling::Linear {
                slope: 20.0,
                intercept: 15.0,
                scaler: Some(0)
            }
        );
    }

    #[test]
    fn other_scales_are_named() {
        let props = vec![
            prop("NI_Number_Of_Scales", PropertyValue::U32(1)),
            prop(
                "NI_Scale[0]_Scale_Type",
                PropertyValue::String("Thermocouple".into()),
            ),
        ];
        assert_eq!(
            Scaling::of(&props, false),
            Scaling::Unsupported("Thermocouple".into())
        );
        assert_eq!(Scaling::of(&[], false), Scaling::None);
    }
}
