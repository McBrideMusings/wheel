//! What a client may ask of the wheeld it reached (ADR-0050). `status` carries `ops`, the op names
//! of every [`ClientMsg`] that wheeld handles, and `compat`, its compatibility level. A client
//! connects when the levels are equal and sends an op only when the server listed it, so a server
//! that is merely older than the client (it lacks ops added since) still serves everything it has.

use crate::ClientMsg;
use serde::Deserialize;
use std::collections::BTreeSet;

/// Every change that broke the protocol, as (level, what changed). A level is added only when an
/// op is removed, or when an op or field a client relies on changes meaning, or when a reply gains
/// a field an older client cannot do without. Adding an op, or a field with a default, is never one.
pub const BREAKS: &[(u32, &str)] = &[
    (1, "wheeld lists the operations it handles"),
    (2, "an orchestrator is an agent pane: orchestrator_start makes one (a global one in the Orchestrator space) and names its pane, and orchestrator_stop and the inbox ops take that pane"),
    (3, "an orchestrator's inbox is files in its folder: inbox_list, inbox_ack and inbox_reply are removed"),
    (4, "wheeld holds no orchestrator: the orchestrator_* ops, the Orchestrator space, agent_send's and pane_start's from, and push_send's task are removed"),
    (5, "wheeld runs no MCP artifacts: artifact_tools, artifact_call and artifact_read, their replies tools, tool_result and resource, and mcp_updated are removed"),
];

/// This build's compatibility level: the last entry of [`BREAKS`].
pub const COMPAT: u32 = BREAKS[BREAKS.len() - 1].0;

/// The op names of every [`ClientMsg`] variant, as serde writes them in `op`, in declaration
/// order. Read from serde's own list of the enum's variants, so a new variant is in it with no
/// edit here.
pub fn ops() -> &'static [&'static str] {
    static OPS: std::sync::OnceLock<&'static [&'static str]> = std::sync::OnceLock::new();
    OPS.get_or_init(|| {
        // A tag that names no variant makes serde report its list of variant names through
        // `Error::unknown_variant`; this error type keeps that list.
        #[derive(Debug)]
        struct Variants(Option<&'static [&'static str]>);
        impl std::fmt::Display for Variants {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("variants")
            }
        }
        impl std::error::Error for Variants {}
        impl serde::de::Error for Variants {
            fn custom<T: std::fmt::Display>(_: T) -> Self {
                Variants(None)
            }
            fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Self {
                Variants(Some(expected))
            }
        }
        let probe = serde::de::value::MapDeserializer::<_, Variants>::new(std::iter::once(("op", "")));
        match ClientMsg::deserialize(probe) {
            Err(Variants(Some(names))) => names,
            _ => panic!("serde did not report ClientMsg's variants"),
        }
    })
}

/// The op a serialized [`ClientMsg`] names; None for text that is not one.
pub fn op_of(frame: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Head {
        op: String,
    }
    serde_json::from_str::<Head>(frame).ok().map(|head| head.op)
}

/// What a wheeld's `status` says it offers. A server from before ops were listed reads as level 0
/// with no ops.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Offer {
    #[serde(default)]
    pub compat: u32,
    #[serde(default = "crate::first_protocol")]
    pub protocol: u32,
    #[serde(default)]
    pub ops: BTreeSet<String>,
}

impl Offer {
    /// This build's own offer: what a wheeld built from the same source reports.
    pub fn ours() -> Self {
        Self {
            compat: COMPAT,
            protocol: crate::PROTOCOL_VERSION,
            ops: ops().iter().map(|op| (*op).to_owned()).collect(),
        }
    }

    /// The offer in a `status` frame's text; None for any other frame.
    pub fn read(frame: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct Head {
            ev: String,
            #[serde(flatten)]
            offer: Offer,
        }
        serde_json::from_str::<Head>(frame)
            .ok()
            .filter(|head| head.ev == "status")
            .map(|head| head.offer)
    }

    /// Whether this build can talk to the server at all: the levels must be equal. A server at a
    /// lower level lacks a change this build relies on; one at a higher level changed something
    /// this build does not know.
    pub fn check(&self) -> Result<(), Mismatch> {
        if self.compat == COMPAT {
            Ok(())
        } else {
            Err(Mismatch {
                server: self.compat,
                client: COMPAT,
            })
        }
    }

    pub fn has(&self, op: &str) -> bool {
        self.ops.contains(op)
    }

    /// The ops this build knows that the server did not list, in declaration order.
    pub fn lacks(&self) -> Vec<&'static str> {
        ops().iter().copied().filter(|op| !self.has(op)).collect()
    }

    /// Whether a serialized [`ClientMsg`] may be sent: its op must be one the server listed.
    /// Text that names no op passes; the server answers it as it would.
    pub fn admit(&self, frame: &str) -> Result<(), Refused> {
        match op_of(frame) {
            Some(op) if !self.has(&op) => Err(Refused { op }),
            _ => Ok(()),
        }
    }
}

/// The server and this build are at different compatibility levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub server: u32,
    pub client: u32,
}

impl Mismatch {
    /// Whether the server is the older side, so restarting it into the bundled one fixes this.
    pub fn server_older(&self) -> bool {
        self.server < self.client
    }

    /// One line naming which side is behind.
    pub fn headline(&self) -> &'static str {
        if self.server_older() {
            "This Wheel needs a newer wheeld"
        } else {
            "This wheeld is newer than this Wheel"
        }
    }

    /// What differs: the levels, and for an older server what each level since added.
    pub fn detail(&self) -> String {
        if self.server_older() {
            let since: Vec<String> = BREAKS
                .iter()
                .filter(|(level, _)| *level > self.server && *level <= self.client)
                .map(|(level, what)| format!("level {level}: {what}"))
                .collect();
            format!(
                "wheeld is at compatibility level {}, this Wheel needs {} ({})",
                self.server,
                self.client,
                since.join("; ")
            )
        } else {
            format!(
                "wheeld is at compatibility level {}, this Wheel at {}",
                self.server, self.client
            )
        }
    }
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.headline(), self.detail())
    }
}

/// A message whose op the server did not list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub op: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the running wheeld does not handle `{}`; restart wheeld to use it",
            self.op
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ops_are_every_client_msg_variant() {
        let ops = ops();
        assert_eq!(ops[0], "status", "declaration order");
        assert!(ops.contains(&"pane_reload") && ops.contains(&"term_open"), "{ops:?}");
        assert_eq!(ops.iter().collect::<BTreeSet<_>>().len(), ops.len(), "no op twice");
        // Each name is one serde takes as a variant: a frame naming it fails, if at all, on its
        // fields, never as an unknown variant.
        for op in ops {
            if let Err(error) = serde_json::from_str::<ClientMsg>(&format!(r#"{{"op":"{op}"}}"#)) {
                assert!(!error.to_string().contains("unknown variant"), "{op}: {error}");
            }
        }
        // Every variant is named: a message of each kind the tests build reads back to a listed op.
        for message in [
            ClientMsg::Status { req: None },
            ClientMsg::SplitRatio {
                req: None,
                split: "x".into(),
                ratio: 0.5,
            },
        ] {
            let op = op_of(&serde_json::to_string(&message).unwrap()).unwrap();
            assert!(ops.contains(&op.as_str()), "{op}");
        }
    }

    #[test]
    fn a_status_carries_its_offer_and_an_older_one_reads_as_level_0() {
        let old = r#"{"ev":"status","protocol":52,"version":"x","spaces":[]}"#;
        assert_eq!(
            Offer::read(old),
            Some(Offer {
                compat: 0,
                protocol: 52,
                ops: BTreeSet::new()
            })
        );
        assert_eq!(Offer::read(r#"{"ev":"spaces"}"#), None);
        let new = r#"{"ev":"status","protocol":53,"compat":1,"ops":["status","term_open"]}"#;
        let offer = Offer::read(new).unwrap();
        assert_eq!((offer.compat, offer.protocol), (1, 53));
        assert!(offer.has("term_open") && !offer.has("pane_reload"));
    }

    #[test]
    fn an_older_server_missing_an_op_connects_and_only_that_op_is_refused() {
        let mut older = Offer::ours();
        older.protocol -= 1;
        older.ops.remove("pane_reload");
        assert_eq!(older.check(), Ok(()));
        assert_eq!(older.lacks(), vec!["pane_reload"]);
        let reload = serde_json::to_string(&ClientMsg::PaneReload {
            req: Some(4),
            pane: "p1".into(),
            set: Default::default(),
            unset: vec![],
        })
        .unwrap();
        let refused = older.admit(&reload).unwrap_err();
        assert_eq!(refused.op, "pane_reload");
        assert!(refused.to_string().contains("`pane_reload`"), "{refused}");
        assert_eq!(
            older.admit(&serde_json::to_string(&ClientMsg::Status { req: None }).unwrap()),
            Ok(())
        );
    }

    #[test]
    fn a_breaking_level_refuses_both_ways_and_says_what_changed() {
        let mut before = Offer::ours();
        before.compat = 0;
        let mismatch = before.check().unwrap_err();
        assert!(mismatch.server_older());
        assert_eq!(mismatch.headline(), "This Wheel needs a newer wheeld");
        assert!(
            mismatch
                .detail()
                .contains("level 1: wheeld lists the operations it handles"),
            "{mismatch}"
        );
        let mut after = Offer::ours();
        after.compat = COMPAT + 1;
        let mismatch = after.check().unwrap_err();
        assert!(!mismatch.server_older());
        assert_eq!(mismatch.headline(), "This wheeld is newer than this Wheel");
        assert_eq!(Offer::ours().check(), Ok(()));
    }
}
