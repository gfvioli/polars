use bitflags::bitflags;
use polars_core::prelude::*;
use polars_core::utils::SuperTypeOptions;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::plans::PlSmallStr;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "ir_serde", derive(Serialize, Deserialize))]
pub struct DistinctOptionsIR {
    /// Subset of columns that will be taken into account.
    pub subset: Option<Arc<[PlSmallStr]>>,
    /// This will maintain the order of the input.
    /// Note that this is more expensive.
    /// `maintain_order` is not supported in the streaming
    /// engine.
    pub maintain_order: bool,
    /// Which rows to keep.
    pub keep_strategy: UniqueKeepStrategy,
    /// Take only a slice of the result
    pub slice: Option<(i64, usize)>,
}

// a boolean that can only be set to `false` safely
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "dsl-schema", derive(schemars::JsonSchema))]
pub struct UnsafeBool(bool);
impl Default for UnsafeBool {
    fn default() -> Self {
        UnsafeBool(true)
    }
}

#[cfg(feature = "dsl-schema")]
impl schemars::JsonSchema for FunctionFlags {
    fn schema_name() -> String {
        "FunctionFlags".to_owned()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(concat!(module_path!(), "::", "FunctionFlags"))
    }

    fn json_schema(_generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        use serde_json::{Map, Value};

        let name_to_bits: Map<String, Value> = Self::all()
            .iter_names()
            .map(|(name, flag)| (name.to_owned(), flag.bits().into()))
            .collect();

        schemars::schema::Schema::Object(schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            format: Some("bitflags".to_owned()),
            extensions: schemars::Map::from_iter([
                // Add a map of flag names and bit patterns to detect schema changes
                ("bitflags".to_owned(), Value::Object(name_to_bits)),
            ]),
            ..Default::default()
        })
    }
}

bitflags!(
        #[repr(transparent)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        pub struct FunctionFlags: u16 {
            /// The physical expression may rename the output of this function.
            /// If set to `false` the physical engine will ensure the left input
            /// expression is the output name.
            const ALLOW_RENAME = 1 << 0;
            /// if set, then the `Series` passed to the function in the group_by operation
            /// will ensure the name is set. This is an extra heap allocation per group.
            const PASS_NAME_TO_APPLY = 1 << 1;
            /// There can be two ways of expanding wildcards:
            ///
            /// Say the schema is 'a', 'b' and there is a function `f`. In this case, `f('*')` can expand
            /// to:
            /// 1. `f('a', 'b')`
            /// 2. `f('a'), f('b')`
            ///
            /// Setting this to true, will lead to behavior 1.
            ///
            /// This also accounts for regex expansion.
            const INPUT_WILDCARD_EXPANSION = 1 << 2;
            /// Automatically explode on unit length if it ran as final aggregation.
            ///
            /// this is the case for aggregations like sum, min, covariance etc.
            /// We need to know this because we cannot see the difference between
            /// the following functions based on the output type and number of elements:
            ///
            /// x: {1, 2, 3}
            ///
            /// head_1(x) -> {1}
            /// sum(x) -> {4}
            ///
            /// mutually exclusive with `RETURNS_SCALAR`
            const RETURNS_SCALAR = 1 << 3;
            /// This can happen with UDF's that use Polars within the UDF.
            /// This can lead to recursively entering the engine and sometimes deadlocks.
            /// This flag must be set to handle that.
            const OPTIONAL_RE_ENTRANT = 1 << 4;
            /// Whether this function allows no inputs.
            const ALLOW_EMPTY_INPUTS = 1 << 5;

            /// Given a function f and a column of values [v1, ..., vn]
            /// f is row-separable i.f.f.
            /// f([v1, ..., vn]) = concat(f(v1, ... vm), f(vm+1, ..., vn))
            const ROW_SEPARABLE = 1 << 6;
            /// Given a function f and a column of values [v1, ..., vn]
            /// f is length preserving i.f.f. len(f([v1, ..., vn])) = n
            ///
            /// mutually exclusive with `RETURNS_SCALAR`
            const LENGTH_PRESERVING = 1 << 7;
            /// NULLs on the first input are propagated to the output.
            const PRESERVES_NULL_FIRST_INPUT = 1 << 8;
            /// NULLs on any input are propagated to the output.
            const PRESERVES_NULL_ALL_INPUTS = 1 << 9;
        }
);

impl FunctionFlags {
    pub fn set_elementwise(&mut self) {
        *self |= Self::ROW_SEPARABLE | Self::LENGTH_PRESERVING;
    }

    pub fn is_elementwise(self) -> bool {
        self.contains(Self::ROW_SEPARABLE | Self::LENGTH_PRESERVING)
    }

    pub fn is_row_separable(self) -> bool {
        self.contains(Self::ROW_SEPARABLE)
    }

    pub fn is_length_preserving(self) -> bool {
        self.contains(Self::LENGTH_PRESERVING)
    }

    pub fn returns_scalar(self) -> bool {
        self.contains(Self::RETURNS_SCALAR)
    }
}

impl Default for FunctionFlags {
    fn default() -> Self {
        Self::from_bits_truncate(0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum CastingRules {
    /// Whether information may be lost during cast. E.g. a float to int is considered lossy,
    /// whereas int to int is considered lossless.
    /// Overflowing is not considered in this flag, that's handled in `strict` casting
    FirstArgLossless,
    Supertype(SuperTypeOptions),
}

impl CastingRules {
    pub fn cast_to_supertypes() -> CastingRules {
        Self::Supertype(Default::default())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(any(feature = "serde"), derive(Serialize, Deserialize))]
#[cfg_attr(feature = "dsl-schema", derive(schemars::JsonSchema))]
pub struct FunctionOptions {
    // Validate the output of a `map`.
    // this should always be true or we could OOB
    pub check_lengths: UnsafeBool,
    pub flags: FunctionFlags,

    /// Options used when deciding how to cast the arguments of the function.
    #[cfg_attr(any(feature = "serde", feature = "dsl-schema"), serde(skip))]
    pub cast_options: Option<CastingRules>,
}

impl FunctionOptions {
    #[cfg(feature = "fused")]
    pub(crate) unsafe fn no_check_lengths(&mut self) {
        self.check_lengths = UnsafeBool(false);
    }
    pub fn check_lengths(&self) -> bool {
        self.check_lengths.0
    }

    pub fn set_elementwise(&mut self) {
        self.flags.set_elementwise();
    }

    pub fn is_elementwise(&self) -> bool {
        self.flags.is_elementwise()
    }

    pub fn is_length_preserving(&self) -> bool {
        self.flags.contains(FunctionFlags::LENGTH_PRESERVING)
    }

    pub fn is_row_separable(&self) -> bool {
        self.flags.is_row_separable()
    }

    pub fn returns_scalar(&self) -> bool {
        self.flags.returns_scalar()
    }

    pub fn elementwise() -> FunctionOptions {
        FunctionOptions {
            ..Default::default()
        }
        .with_flags(|f| f | FunctionFlags::ROW_SEPARABLE | FunctionFlags::LENGTH_PRESERVING)
    }

    pub fn elementwise_with_infer() -> FunctionOptions {
        Self::length_preserving()
    }

    pub fn row_separable() -> FunctionOptions {
        FunctionOptions {
            ..Default::default()
        }
        .with_flags(|f| f | FunctionFlags::ROW_SEPARABLE)
    }

    pub fn length_preserving() -> FunctionOptions {
        FunctionOptions {
            ..Default::default()
        }
        .with_flags(|f| f | FunctionFlags::LENGTH_PRESERVING)
    }

    pub fn groupwise() -> FunctionOptions {
        FunctionOptions {
            ..Default::default()
        }
    }

    pub fn aggregation() -> FunctionOptions {
        let mut options = Self::groupwise();
        options.flags |= FunctionFlags::RETURNS_SCALAR;
        options
    }

    pub fn with_supertyping(self, supertype_options: SuperTypeOptions) -> FunctionOptions {
        self.with_casting_rules(CastingRules::Supertype(supertype_options))
    }

    pub fn with_casting_rules(mut self, casting_rules: CastingRules) -> FunctionOptions {
        self.cast_options = Some(casting_rules);
        self
    }

    pub fn with_flags(mut self, f: impl Fn(FunctionFlags) -> FunctionFlags) -> FunctionOptions {
        self.flags = f(self.flags);
        self
    }
}

impl Default for FunctionOptions {
    fn default() -> Self {
        FunctionOptions {
            check_lengths: UnsafeBool(true),
            cast_options: Default::default(),
            flags: Default::default(),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "dsl-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionOptions {
    pub run_parallel: bool,
    pub duplicate_check: bool,
    // Should length-1 Series be broadcast to the length of the dataframe.
    // Only used by CSE optimizer
    pub should_broadcast: bool,
}

impl Default for ProjectionOptions {
    fn default() -> Self {
        Self {
            run_parallel: true,
            duplicate_check: true,
            should_broadcast: true,
        }
    }
}

impl ProjectionOptions {
    /// Conservatively merge the options of two [`ProjectionOptions`]
    pub fn merge_options(&self, other: &Self) -> Self {
        Self {
            run_parallel: self.run_parallel & other.run_parallel,
            duplicate_check: self.duplicate_check & other.duplicate_check,
            should_broadcast: self.should_broadcast | other.should_broadcast,
        }
    }
}
