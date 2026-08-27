use agent_client_protocol_schema::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome,
};
use maki_agent::permissions::PermissionAnswer;

const ALLOW_ONCE_ID: &str = "allow_once";
const ALLOW_ALWAYS_ID: &str = "allow_always";
const REJECT_ONCE_ID: &str = "reject_once";
const REJECT_ALWAYS_ID: &str = "reject_always";

/// Permissions the client may grant. A forced prompt omits the ongoing
/// "Allow always" option: ACP surfaces options, not a suggestible scope text,
/// so dropping it is the faithful analog of the UI hiding its allow suggestion
/// for a command with a short-circuiting construct.
pub fn permission_options(force_prompt: bool) -> Vec<PermissionOption> {
    let mut options = vec![
        PermissionOption::new(
            PermissionOptionId::from(ALLOW_ONCE_ID),
            "Allow once",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            PermissionOptionId::from(REJECT_ONCE_ID),
            "Reject once",
            PermissionOptionKind::RejectOnce,
        ),
        PermissionOption::new(
            PermissionOptionId::from(REJECT_ALWAYS_ID),
            "Reject always",
            PermissionOptionKind::RejectAlways,
        ),
    ];
    if !force_prompt {
        options.insert(
            1,
            PermissionOption::new(
                PermissionOptionId::from(ALLOW_ALWAYS_ID),
                "Allow always",
                PermissionOptionKind::AllowAlways,
            ),
        );
    }
    options
}

pub fn outcome_to_answer(outcome: &RequestPermissionOutcome) -> PermissionAnswer {
    match outcome {
        RequestPermissionOutcome::Cancelled => PermissionAnswer::Deny,
        RequestPermissionOutcome::Selected(selected) => match selected.option_id.0.as_ref() {
            ALLOW_ONCE_ID => PermissionAnswer::AllowOnce,
            ALLOW_ALWAYS_ID => PermissionAnswer::AllowSession,
            REJECT_ONCE_ID => PermissionAnswer::Deny,
            REJECT_ALWAYS_ID => PermissionAnswer::DenyAlwaysLocal,
            _ => PermissionAnswer::Deny,
        },
        _ => PermissionAnswer::Deny,
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(false ; "no_force_keeps_allow_always")]
    #[test_case(true ; "force_drops_allow_always")]
    fn permission_options_respect_force_prompt(force: bool) {
        let options = permission_options(force);
        let ids: Vec<&str> = options.iter().map(|o| o.option_id.0.as_ref()).collect();
        let has_allow_always = ids.contains(&ALLOW_ALWAYS_ID);
        assert_eq!(has_allow_always, !force);
        assert!(ids.contains(&ALLOW_ONCE_ID));
        assert!(ids.contains(&REJECT_ONCE_ID));
        assert!(ids.contains(&REJECT_ALWAYS_ID));
    }
}
