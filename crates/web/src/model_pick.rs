/// One layer of a model choice: `(model, effort)`. `None` leaves a field to the layers below;
/// an empty string is an explicit pick of the runtime's own default.
pub type Layer = (Option<String>, Option<String>);

/// The choice a conversation runs with: each field from the first layer that sets it.
pub fn layered(layers: &[Option<&Layer>]) -> Layer {
    let mut model = None;
    let mut effort = None;
    for (m, e) in layers.iter().flatten() {
        if model.is_none() {
            model.clone_from(m);
        }
        if effort.is_none() {
            effort.clone_from(e);
        }
    }
    (model, effort)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(m: Option<&str>, e: Option<&str>) -> Layer {
        (m.map(str::to_owned), e.map(str::to_owned))
    }

    #[test]
    fn each_field_comes_from_the_first_layer_that_sets_it() {
        let thread = l(Some("haiku"), None);
        let ws = l(Some("opus"), Some("high"));
        let global = l(Some("sonnet"), Some("low"));
        assert_eq!(
            layered(&[Some(&thread), Some(&ws), Some(&global)]),
            l(Some("haiku"), Some("high"))
        );
        assert_eq!(layered(&[None, Some(&ws), Some(&global)]), ws);
        assert_eq!(layered(&[None, None, Some(&global)]), global);
        assert_eq!(layered(&[None, None, None]), l(None, None));
    }

    #[test]
    fn an_explicit_default_is_not_filled_from_below() {
        let thread = l(Some(""), Some(""));
        let ws = l(Some("opus"), Some("high"));
        assert_eq!(layered(&[Some(&thread), Some(&ws)]), l(Some(""), Some("")));
    }
}
