mod dot;
mod format;
mod inputs;
mod schema;
pub(crate) mod tree_format;

use std::borrow::Cow;
use std::fmt;

pub use dot::{EscapeLabel, IRDotDisplay, PathsDisplay, ScanSourcesDisplay};
pub use format::{ExprIRDisplay, IRDisplay, write_group_by, write_ir_non_recursive};
use polars_core::prelude::*;
use polars_utils::idx_vec::UnitVec;
use polars_utils::unique_id::UniqueId;
use polars_utils::unitvec;
#[cfg(feature = "ir_serde")]
use serde::{Deserialize, Serialize};
use strum_macros::IntoStaticStr;

use self::hive::HivePartitionsDf;
use crate::prelude::*;

#[cfg_attr(feature = "ir_serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IRPlan {
    pub lp_top: Node,
    pub lp_arena: Arena<IR>,
    pub expr_arena: Arena<AExpr>,
}

#[derive(Clone, Copy)]
pub struct IRPlanRef<'a> {
    pub lp_top: Node,
    pub lp_arena: &'a Arena<IR>,
    pub expr_arena: &'a Arena<AExpr>,
}

/// [`IR`] is a representation of [`DslPlan`] with [`Node`]s which are allocated in an [`Arena`]
/// In this IR the logical plan has access to the full dataset.
#[derive(Clone, Debug, Default, IntoStaticStr)]
#[cfg_attr(feature = "ir_serde", derive(Serialize, Deserialize))]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum IR {
    #[cfg(feature = "python")]
    PythonScan {
        options: PythonOptions,
    },
    Slice {
        input: Node,
        offset: i64,
        len: IdxSize,
    },
    Filter {
        input: Node,
        predicate: ExprIR,
    },
    Scan {
        sources: ScanSources,
        file_info: FileInfo,
        hive_parts: Option<HivePartitionsDf>,
        predicate: Option<ExprIR>,
        /// schema of the projected file
        output_schema: Option<SchemaRef>,
        scan_type: Box<FileScanIR>,
        /// generic options that can be used for all file types.
        unified_scan_args: Box<UnifiedScanArgs>,
    },
    DataFrameScan {
        df: Arc<DataFrame>,
        schema: SchemaRef,
        // Schema of the projected file
        // If `None`, no projection is applied
        output_schema: Option<SchemaRef>,
    },
    // Only selects columns (semantically only has row access).
    // This is a more restricted operation than `Select`.
    SimpleProjection {
        input: Node,
        columns: SchemaRef,
    },
    // Polars' `select` operation. This may access full materialized data.
    Select {
        input: Node,
        expr: Vec<ExprIR>,
        schema: SchemaRef,
        options: ProjectionOptions,
    },
    Sort {
        input: Node,
        by_column: Vec<ExprIR>,
        slice: Option<(i64, usize)>,
        sort_options: SortMultipleOptions,
    },
    Cache {
        input: Node,
        /// This holds the `Arc<DslPlan>` to guarantee uniqueness.
        id: UniqueId,
        /// How many hits the cache must be saved in memory.
        cache_hits: u32,
    },
    GroupBy {
        input: Node,
        keys: Vec<ExprIR>,
        aggs: Vec<ExprIR>,
        schema: SchemaRef,
        maintain_order: bool,
        options: Arc<GroupbyOptions>,
        #[cfg_attr(feature = "ir_serde", serde(skip))]
        apply: Option<Arc<dyn DataFrameUdf>>,
    },
    Join {
        input_left: Node,
        input_right: Node,
        schema: SchemaRef,
        left_on: Vec<ExprIR>,
        right_on: Vec<ExprIR>,
        options: Arc<JoinOptionsIR>,
    },
    HStack {
        input: Node,
        exprs: Vec<ExprIR>,
        schema: SchemaRef,
        options: ProjectionOptions,
    },
    Distinct {
        input: Node,
        options: DistinctOptionsIR,
    },
    MapFunction {
        input: Node,
        function: FunctionIR,
    },
    Union {
        inputs: Vec<Node>,
        options: UnionOptions,
    },
    /// Horizontal concatenation
    /// - Invariant: the names will be unique
    HConcat {
        inputs: Vec<Node>,
        schema: SchemaRef,
        options: HConcatOptions,
    },
    ExtContext {
        input: Node,
        contexts: Vec<Node>,
        schema: SchemaRef,
    },
    Sink {
        input: Node,
        payload: SinkTypeIR,
    },
    /// Node that allows for multiple plans to be executed in parallel with common subplan
    /// elimination and everything.
    SinkMultiple {
        inputs: Vec<Node>,
    },
    #[cfg(feature = "merge_sorted")]
    MergeSorted {
        input_left: Node,
        input_right: Node,
        key: PlSmallStr,
    },
    #[default]
    Invalid,
}

impl IRPlan {
    pub fn new(top: Node, ir_arena: Arena<IR>, expr_arena: Arena<AExpr>) -> Self {
        Self {
            lp_top: top,
            lp_arena: ir_arena,
            expr_arena,
        }
    }

    pub fn root(&self) -> &IR {
        self.lp_arena.get(self.lp_top)
    }

    pub fn as_ref(&self) -> IRPlanRef<'_> {
        IRPlanRef {
            lp_top: self.lp_top,
            lp_arena: &self.lp_arena,
            expr_arena: &self.expr_arena,
        }
    }

    pub fn describe(&self) -> String {
        self.as_ref().describe()
    }

    pub fn describe_tree_format(&self) -> String {
        self.as_ref().describe_tree_format()
    }

    pub fn display(&self) -> format::IRDisplay<'_> {
        self.as_ref().display()
    }

    pub fn display_dot(&self) -> dot::IRDotDisplay<'_> {
        self.as_ref().display_dot()
    }
}

impl<'a> IRPlanRef<'a> {
    pub fn root(self) -> &'a IR {
        self.lp_arena.get(self.lp_top)
    }

    pub fn with_root(self, root: Node) -> Self {
        Self {
            lp_top: root,
            lp_arena: self.lp_arena,
            expr_arena: self.expr_arena,
        }
    }

    pub fn display(self) -> format::IRDisplay<'a> {
        format::IRDisplay::new(self)
    }

    pub fn display_dot(self) -> dot::IRDotDisplay<'a> {
        dot::IRDotDisplay::new(self)
    }

    pub fn describe(self) -> String {
        self.display().to_string()
    }

    pub fn describe_tree_format(self) -> String {
        let mut visitor = tree_format::TreeFmtVisitor::default();
        tree_format::TreeFmtNode::root_logical_plan(self).traverse(&mut visitor);
        format!("{visitor:#?}")
    }
}

impl fmt::Debug for IRPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <format::IRDisplay as fmt::Display>::fmt(&self.display(), f)
    }
}

impl fmt::Debug for IRPlanRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <format::IRDisplay as fmt::Display>::fmt(&self.display(), f)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // skipped for now
    #[ignore]
    #[test]
    fn test_alp_size() {
        assert!(size_of::<IR>() <= 152);
    }
}
