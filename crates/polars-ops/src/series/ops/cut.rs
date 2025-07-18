use polars_compute::rolling::QuantileMethod;
use polars_core::chunked_array::builder::CategoricalChunkedBuilder;
use polars_core::prelude::*;
use polars_utils::format_pl_smallstr;

fn map_cats(
    s: &Series,
    labels: &[PlSmallStr],
    sorted_breaks: &[f64],
    left_closed: bool,
    include_breaks: bool,
) -> PolarsResult<Series> {
    let out_name = PlSmallStr::from_static("category");

    let s2 = s.cast(&DataType::Float64)?;
    // It would be nice to parallelize this
    let s_iter = s2.f64()?.into_iter();

    let op = if left_closed {
        PartialOrd::ge
    } else {
        PartialOrd::gt
    };

    if include_breaks {
        // This is to replicate the behavior of the old buggy version that only worked on series and
        // returned a dataframe. That included a column of the right endpoint of the interval. So we
        // return a struct series instead which can be turned into a dataframe later.
        let right_ends = [sorted_breaks, &[f64::INFINITY]].concat();
        let mut bld = CategoricalChunkedBuilder::<Categorical32Type>::new(
            out_name.clone(),
            DataType::from_categories(Categories::global()),
        );
        let mut brk_vals = PrimitiveChunkedBuilder::<Float64Type>::new(
            PlSmallStr::from_static("breakpoint"),
            s.len(),
        );
        s_iter
            .map(|opt| {
                opt.filter(|x| !x.is_nan())
                    .map(|x| sorted_breaks.partition_point(|v| op(&x, v)))
            })
            .for_each(|idx| match idx {
                None => {
                    bld.append_null();
                    brk_vals.append_null();
                },
                Some(idx) => unsafe {
                    bld.append_str(labels.get_unchecked(idx)).unwrap();
                    brk_vals.append_value(*right_ends.get_unchecked(idx));
                },
            });

        let outvals = [brk_vals.finish().into_series(), bld.finish().into_series()];
        Ok(StructChunked::from_series(out_name, outvals[0].len(), outvals.iter())?.into_series())
    } else {
        Ok(CategoricalChunked::<Categorical32Type>::from_str_iter(
            out_name,
            DataType::from_categories(Categories::global()),
            s_iter.map(|opt| {
                opt.filter(|x| !x.is_nan()).map(|x| {
                    let pt = sorted_breaks.partition_point(|v| op(&x, v));
                    unsafe { labels.get_unchecked(pt).as_str() }
                })
            }),
        )?
        .into_series())
    }
}

pub fn compute_labels(breaks: &[f64], left_closed: bool) -> PolarsResult<Vec<PlSmallStr>> {
    let lo = std::iter::once(&f64::NEG_INFINITY).chain(breaks.iter());
    let hi = breaks.iter().chain(std::iter::once(&f64::INFINITY));

    let ret = lo
        .zip(hi)
        .map(|(l, h)| {
            if left_closed {
                format_pl_smallstr!("[{}, {})", l, h)
            } else {
                format_pl_smallstr!("({}, {}]", l, h)
            }
        })
        .collect();
    Ok(ret)
}

pub fn cut(
    s: &Series,
    mut breaks: Vec<f64>,
    labels: Option<Vec<PlSmallStr>>,
    left_closed: bool,
    include_breaks: bool,
) -> PolarsResult<Series> {
    // Breaks must be sorted to cut inputs properly.
    polars_ensure!(!breaks.iter().any(|x| x.is_nan()), ComputeError: "breaks cannot be NaN");
    breaks.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

    polars_ensure!(breaks.windows(2).all(|x| x[0] != x[1]), Duplicate: "breaks are not unique");
    if !breaks.is_empty() {
        polars_ensure!(breaks[0] > f64::NEG_INFINITY, ComputeError: "don't include -inf in breaks");
        polars_ensure!(breaks[breaks.len() - 1] < f64::INFINITY, ComputeError: "don't include inf in breaks");
    }

    let cut_labels = if let Some(l) = labels {
        polars_ensure!(l.len() == breaks.len() + 1, ShapeMismatch: "provide len(quantiles) + 1 labels");
        l
    } else {
        compute_labels(&breaks, left_closed)?
    };
    map_cats(s, &cut_labels, &breaks, left_closed, include_breaks)
}

pub fn qcut(
    s: &Series,
    probs: Vec<f64>,
    labels: Option<Vec<PlSmallStr>>,
    left_closed: bool,
    allow_duplicates: bool,
    include_breaks: bool,
) -> PolarsResult<Series> {
    polars_ensure!(!probs.iter().any(|x| x.is_nan()), ComputeError: "quantiles cannot be NaN");

    if s.null_count() == s.len() {
        // If we only have nulls we don't have any breakpoints.
        return Ok(Series::full_null(
            s.name().clone(),
            s.len(),
            &DataType::from_categories(Categories::global()),
        ));
    }

    let s = s.cast(&DataType::Float64)?;
    let s2 = s.sort(SortOptions::default())?;
    let ca = s2.f64()?;

    let f = |&p| ca.quantile(p, QuantileMethod::Linear).unwrap().unwrap();
    let mut qbreaks: Vec<_> = probs.iter().map(f).collect();
    qbreaks.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

    if !allow_duplicates {
        polars_ensure!(qbreaks.windows(2).all(|x| x[0] != x[1]), Duplicate: "quantiles are not unique while allow_duplicates=False");
    }

    let cut_labels = if let Some(l) = labels {
        polars_ensure!(l.len() == qbreaks.len() + 1, ShapeMismatch: "provide len(quantiles) + 1 labels");
        l
    } else {
        compute_labels(&qbreaks, left_closed)?
    };

    map_cats(&s, &cut_labels, &qbreaks, left_closed, include_breaks)
}

mod test {
    // This need metadata in fields
    #[ignore]
    #[test]
    fn test_map_cats_fast_unique() {
        // This test is here to check the fast unique flag is set when it can be
        // as it is not visible to Python.
        use polars_core::prelude::*;

        use super::map_cats;

        let s = Series::new("x".into(), &[1, 2, 3, 4, 5]);

        let labels = &["a", "b", "c"].map(PlSmallStr::from_static);
        let breaks = &[2.0, 4.0];
        let left_closed = false;

        let include_breaks = false;
        let out = map_cats(&s, labels, breaks, left_closed, include_breaks).unwrap();
        out.cat32().unwrap();

        let include_breaks = true;
        let out = map_cats(&s, labels, breaks, left_closed, include_breaks).unwrap();
        let out = out.struct_().unwrap().fields_as_series()[1].clone();
        out.cat32().unwrap();
    }
}
