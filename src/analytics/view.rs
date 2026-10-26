//! What the user asked to leave out of the picture.
//!
//! One value, applied by the snapshot builder before aggregation, so every surface sees
//! the same rows and the totals describe them. The fields grow with the filters; the
//! builder stays the only place that applies them.

/// The active view filters. Default is "show everything", which must stay
/// byte-identical to a build without any filtering at all.
///
/// Free models are never filtered out: a model that cost nothing is still a row, and its
/// tokens still count towards the totals.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct View {
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
        self.project
            .as_ref()
            .map(|project| format!("project: {project}"))
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
        let only_project = View {
            project: Some("demo".to_string()),
            pinned_models: Vec::new(),
            pinned_providers: Vec::new(),
        };
        assert_eq!(only_project.label(), Some("project: demo".to_string()));
    }
}
