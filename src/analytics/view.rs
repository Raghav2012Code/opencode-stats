//! What the user asked to leave out of the picture.
//!
//! One value, applied by the snapshot builder before aggregation, so every surface sees
//! the same rows and the totals describe them. The fields grow with the filters; the
//! builder stays the only place that applies them.

/// The active view filters. Default is "show everything", which must stay
/// byte-identical to a build without any filtering at all.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct View {
    /// Leave out model rows whose cost over the window is zero.
    pub hide_zero_cost: bool,
    /// Show only this project, matched the way session rows resolve it.
    pub project: Option<String>,
    /// These models come first, in this order; everything else keeps its order.
    pub pinned_models: Vec<String>,
    /// These providers come first, in this order; everything else keeps its order.
    pub pinned_providers: Vec<String>,
}

impl View {
    /// The wording for the on-screen notice, or nothing when no filter is on.
    pub fn label(&self) -> Option<String> {
        match (&self.project, self.hide_zero_cost) {
            (Some(project), true) => Some(format!("project: {project} · hiding zero-cost rows")),
            (Some(project), false) => Some(format!("project: {project}")),
            (None, true) => Some("hiding zero-cost rows".to_string()),
            (None, false) => None,
        }
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
        let only_zero = View {
            hide_zero_cost: true,
            project: None,
            pinned_models: Vec::new(),
            pinned_providers: Vec::new(),
        };
        assert_eq!(only_zero.label(), Some("hiding zero-cost rows".to_string()));

        let only_project = View {
            hide_zero_cost: false,
            project: Some("demo".to_string()),
            pinned_models: Vec::new(),
            pinned_providers: Vec::new(),
        };
        assert_eq!(only_project.label(), Some("project: demo".to_string()));

        let both = View {
            hide_zero_cost: true,
            project: Some("demo".to_string()),
            pinned_models: Vec::new(),
            pinned_providers: Vec::new(),
        };
        assert_eq!(
            both.label(),
            Some("project: demo · hiding zero-cost rows".to_string())
        );
    }
}
