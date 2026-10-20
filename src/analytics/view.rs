//! What the user asked to leave out of the picture.
//!
//! One value, applied by the snapshot builder before aggregation, so every surface sees
//! the same rows and the totals describe them. The fields grow with the filters; the
//! builder stays the only place that applies them.

/// The active view filters. Default is "show everything", which must stay
/// byte-identical to a build without any filtering at all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct View {
    /// Leave out model rows whose cost over the window is zero.
    pub hide_zero_cost: bool,
}

impl View {
    /// The wording for the on-screen notice, or nothing when no filter is on.
    pub fn label(self) -> Option<&'static str> {
        self.hide_zero_cost.then_some("hiding zero-cost rows")
    }
}

#[cfg(test)]
mod tests {
    use super::View;

    #[test]
    fn the_default_view_filters_nothing() {
        let view = View::default();

        assert_eq!(view.label(), None);
    }

    #[test]
    fn an_active_view_names_what_it_hides() {
        let view = View {
            hide_zero_cost: true,
        };

        assert_eq!(view.label(), Some("hiding zero-cost rows"));
    }
}
