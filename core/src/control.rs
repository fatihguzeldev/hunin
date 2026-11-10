use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::project::{
    Error, Project, Reference, Result, bump_revision, encode_bounded, ensure_run_active,
    validate_id,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Launch,
    Attach,
    Stop,
    Cancel,
    Finalize,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Attach => "attach",
            Self::Stop => "stop",
            Self::Cancel => "cancel",
            Self::Finalize => "finalize",
        }
    }
    fn terminal(self) -> bool {
        matches!(self, Self::Stop | Self::Cancel | Self::Finalize)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestId {
    pub project: String,
    pub caller: String,
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestOutcome {
    pub request: RequestId,
    pub run: Reference,
    pub action: String,
    pub arguments: Value,
    pub generation: i64,
    pub state: String,
    pub result: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct AcceptedRequest {
    pub outcome: RequestOutcome,
    pub newly_accepted: bool,
}

#[derive(Clone, Debug)]
pub struct Controller {
    project: Project,
}

impl Controller {
    pub fn new(project: Project) -> Self {
        Self { project }
    }

    fn check(&self, key: &RequestId) -> Result<()> {
        if key.project != self.project.id() {
            return Err(Error::ForeignProject);
        }
        validate_id(&key.caller)?;
        validate_id(&key.id)
    }

    // Request identities are retained for the lifetime of their project, including uncertain effects.
    pub fn accept(
        &self,
        key: &RequestId,
        run: &Reference,
        action: Action,
        arguments: Value,
    ) -> Result<AcceptedRequest> {
        self.check(key)?;
        self.project.check_reference(run)?;
        self.project.check_run(run)?;
        let arguments = encode_bounded(&arguments)?;
        self.project.transaction(|tx| {
            if let Some(existing) = load(tx, key)? {
                let stored: String = tx.query_row("SELECT arguments FROM control_requests WHERE caller=?1 AND id=?2",params![key.caller,key.id],|row|row.get(0))?;
                if stored != arguments || existing.run != *run || existing.action != action.name() {
                    return Err(Error::Conflict("request identity reused with different intent".into()));
                }
                return Ok(AcceptedRequest { outcome: existing, newly_accepted: false });
            }
            tx.execute("INSERT INTO control_scopes(run_id,generation,terminal) VALUES (?1,1,NULL) ON CONFLICT DO NOTHING",[&run.id])?;
            ensure_run_active(tx,&run.id)?;
            if action.terminal() {
                let generation: i64 = tx.query_row("SELECT generation FROM control_scopes WHERE run_id=?1",[&run.id],|row|row.get(0))?;
                if generation == i64::MAX { return Err(Error::Limit("control generation exhausted".into())); }
                tx.execute("UPDATE control_scopes SET generation=generation+1,terminal=?2 WHERE run_id=?1",params![run.id,format!("{}:accepted",action.name())])?;
                tx.execute("UPDATE control_requests SET result=CASE WHEN state='accepted' THEN ?2 ELSE ?3 END,state=CASE WHEN state='accepted' THEN 'cancelled' ELSE 'uncertain' END WHERE run_id=?1 AND state IN ('accepted','dispatched')",params![run.id,encode_bounded(&json!({"reason":"terminal control accepted before dispatch","cleanup_pending":false}))?,encode_bounded(&json!({"reason":"terminal control accepted after dispatch","cleanup_pending":true}))?])?;
            }
            let generation: i64 = tx.query_row("SELECT generation FROM control_scopes WHERE run_id=?1",[&run.id],|row|row.get(0))?;
            tx.execute("INSERT INTO control_requests(caller,id,run_id,action,arguments,generation,state,result) VALUES (?1,?2,?3,?4,?5,?6,'accepted',NULL)",params![key.caller,key.id,run.id,action.name(),arguments,generation])?;
            bump_revision(tx)?;
            Ok(AcceptedRequest { outcome: load(tx,key)?.ok_or_else(||Error::NotFound("accepted request".into()))?, newly_accepted: true })
        })
    }

    pub fn lookup(&self, key: &RequestId) -> Result<RequestOutcome> {
        self.check(key)?;
        self.project.with_connection(|connection| {
            load(connection, key)?.ok_or_else(|| Error::NotFound("control request".into()))
        })
    }

    // This compare-and-set must commit before issuing the external effect, even on a retry.
    pub fn dispatch(&self, key: &RequestId) -> Result<RequestOutcome> {
        self.check(key)?;
        self.project.transaction(|tx| {
            let request =
                load(tx, key)?.ok_or_else(|| Error::NotFound("control request".into()))?;
            let generation: i64 = tx.query_row(
                "SELECT generation FROM control_scopes WHERE run_id=?1",
                [&request.run.id],
                |row| row.get(0),
            )?;
            if request.state != "accepted" || request.generation != generation {
                return Err(Error::Conflict(
                    "request cannot be dispatched again or after terminal control".into(),
                ));
            }
            tx.execute(
                "UPDATE control_requests SET state='dispatched' WHERE caller=?1 AND id=?2",
                params![key.caller, key.id],
            )?;
            bump_revision(tx)?;
            load(tx, key)?.ok_or_else(|| Error::NotFound("dispatched request".into()))
        })
    }

    pub fn complete(
        &self,
        key: &RequestId,
        generation: i64,
        succeeded: bool,
        result: Value,
    ) -> Result<RequestOutcome> {
        self.check(key)?;
        let encoded = encode_bounded(&result)?;
        self.project.transaction(|tx| {
            let request =
                load(tx, key)?.ok_or_else(|| Error::NotFound("control request".into()))?;
            let current: i64 = tx.query_row(
                "SELECT generation FROM control_scopes WHERE run_id=?1",
                [&request.run.id],
                |row| row.get(0),
            )?;
            if request.generation != generation {
                return Err(Error::Conflict("completion generation mismatch".into()));
            }
            if request.state == "succeeded" || request.state == "failed" {
                let expected = if succeeded { "succeeded" } else { "failed" };
                if request.state == expected && request.result.as_ref() == Some(&result) {
                    return Ok(request);
                }
                return Err(Error::Conflict("terminal completion changed".into()));
            }
            if !matches!(request.state.as_str(), "dispatched" | "uncertain") {
                return Err(Error::Conflict("request has no dispatched effect".into()));
            }
            let stale = generation != current || request.state == "uncertain";
            let state = if stale {
                "uncertain"
            } else if succeeded {
                "succeeded"
            } else {
                "failed"
            };
            let body = if stale {
                encode_bounded(&json!({"late_completion":result,"cleanup_pending":true}))?
            } else {
                encoded.clone()
            };
            tx.execute(
                "UPDATE control_requests SET state=?3,result=?4 WHERE caller=?1 AND id=?2",
                params![key.caller, key.id, state, body],
            )?;
            if !stale && matches!(request.action.as_str(), "stop" | "cancel" | "finalize") {
                let terminal = if succeeded {
                    format!("{}:completed", request.action)
                } else {
                    format!("{}:failed", request.action)
                };
                tx.execute(
                    "UPDATE control_scopes SET terminal=?2 WHERE run_id=?1",
                    params![request.run.id, terminal],
                )?;
            }
            bump_revision(tx)?;
            load(tx, key)?.ok_or_else(|| Error::NotFound("completed request".into()))
        })
    }

    pub fn interrupted(&self, key: &RequestId) -> Result<RequestOutcome> {
        self.check(key)?;
        self.project.transaction(|tx| {
            let request = load(tx,key)?.ok_or_else(||Error::NotFound("control request".into()))?;
            if request.state == "dispatched" {
                tx.execute("UPDATE control_requests SET state='uncertain',result=?3 WHERE caller=?1 AND id=?2",params![key.caller,key.id,encode_bounded(&json!({"reason":"effect result lost across interruption","cleanup_pending":true}))?])?;
                bump_revision(tx)?;
            }
            load(tx,key)?.ok_or_else(||Error::NotFound("interrupted request".into()))
        })
    }

    pub fn reconcile(
        &self,
        key: &RequestId,
        evidence: Value,
        cleanup_complete: bool,
    ) -> Result<RequestOutcome> {
        self.check(key)?;
        let body = encode_bounded(
            &json!({"reconciliation":evidence,"cleanup_pending":!cleanup_complete}),
        )?;
        self.project.transaction(|tx| {
            let request =
                load(tx, key)?.ok_or_else(|| Error::NotFound("control request".into()))?;
            if request.state != "uncertain" {
                return Err(Error::Conflict(
                    "only uncertain effects can be reconciled".into(),
                ));
            }
            tx.execute(
                "UPDATE control_requests SET state=?3,result=?4 WHERE caller=?1 AND id=?2",
                params![
                    key.caller,
                    key.id,
                    if cleanup_complete {
                        "reconciled"
                    } else {
                        "uncertain"
                    },
                    body
                ],
            )?;
            bump_revision(tx)?;
            load(tx, key)?.ok_or_else(|| Error::NotFound("reconciled request".into()))
        })
    }

    pub fn run_state(&self, run: &Reference) -> Result<Value> {
        self.project.check_reference(run)?;
        self.project.check_run(run)?;
        self.project.with_connection(|connection| {
            let state: Option<(i64, Option<String>)> = connection
                .query_row(
                    "SELECT generation,terminal FROM control_scopes WHERE run_id=?1",
                    [&run.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (generation, terminal) = state.unwrap_or((1, None));
            Ok(json!({"generation":generation,"terminal":terminal}))
        })
    }
}

fn load(connection: &Connection, key: &RequestId) -> Result<Option<RequestOutcome>> {
    let row: Option<(String,String,String,i64,String,Option<String>)> = connection.query_row("SELECT run_id,action,arguments,generation,state,result FROM control_requests WHERE caller=?1 AND id=?2",params![key.caller,key.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
    row.map(|(run, action, arguments, generation, state, result)| {
        Ok(RequestOutcome {
            request: key.clone(),
            run: Reference {
                project: key.project.clone(),
                id: run,
            },
            action,
            arguments: serde_json::from_str(&arguments)?,
            generation,
            state,
            result: result.map(|body| serde_json::from_str(&body)).transpose()?,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(root: &std::path::Path, project: &str) -> (Project, Controller, Reference, RequestId) {
        let p = Project::create(root, project).unwrap();
        let i = p
            .create_investigation("investigation", "preview", json!({}))
            .unwrap();
        let r = p.create_run(&i, "run", json!({"input":"assets"})).unwrap();
        let key = RequestId {
            project: project.into(),
            caller: "agent".into(),
            id: "launch-1".into(),
        };
        (p.clone(), Controller::new(p), r, key)
    }

    #[test]
    fn request_intent_survives_lost_result_without_repeating_effect() {
        let root = tempfile::tempdir().unwrap();
        let (p, c, run, key) = setup(root.path(), "a");
        let accepted = c
            .accept(&key, &run, Action::Launch, json!({"program":"preview"}))
            .unwrap();
        assert!(accepted.newly_accepted);
        let dispatch = c.dispatch(&key).unwrap();
        drop(c);
        drop(p);
        let c = Controller::new(Project::open(root.path(), "a").unwrap());
        assert!(
            !c.accept(&key, &run, Action::Launch, json!({"program":"preview"}))
                .unwrap()
                .newly_accepted
        );
        assert!(c.dispatch(&key).is_err());
        assert_eq!(c.interrupted(&key).unwrap().state, "uncertain");
        assert!(
            c.accept(&key, &run, Action::Launch, json!({"program":"other"}))
                .is_err()
        );
        let late = c
            .complete(&key, dispatch.generation, true, json!({"pid":123}))
            .unwrap();
        assert_eq!(late.state, "uncertain");
        assert!(late.result.unwrap()["cleanup_pending"].as_bool().unwrap());
    }

    #[test]
    fn terminal_control_fences_late_launch_and_novel_registration() {
        let root = tempfile::tempdir().unwrap();
        let (p, c, run, key) = setup(root.path(), "a");
        c.accept(&key, &run, Action::Launch, json!({})).unwrap();
        let dispatch = c.dispatch(&key).unwrap();
        let cancel = RequestId {
            id: "cancel-1".into(),
            ..key.clone()
        };
        c.accept(&cancel, &run, Action::Cancel, json!({})).unwrap();
        assert_eq!(
            c.complete(&key, dispatch.generation, true, json!({"pid":42}))
                .unwrap()
                .state,
            "uncertain"
        );
        assert!(
            p.register_target(&run, "late", json!({"pid":42}), true)
                .is_err()
        );
        assert!(c.dispatch(&key).is_err());
        assert!(c.dispatch(&cancel).is_ok());
    }

    #[test]
    fn equal_request_ids_remain_project_bound() {
        let root = tempfile::tempdir().unwrap();
        let (_, a, ra, ka) = setup(root.path(), "a");
        let (_, b, rb, kb) = setup(root.path(), "b");
        a.accept(&ka, &ra, Action::Launch, json!({"a":1})).unwrap();
        b.accept(&kb, &rb, Action::Launch, json!({"b":2})).unwrap();
        assert!(b.lookup(&ka).is_err());
        assert!(a.accept(&ka, &rb, Action::Launch, json!({})).is_err());
        assert_eq!(b.lookup(&kb).unwrap().run.project, "b");
    }
}
