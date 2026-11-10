use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Captured,
    Instrumented,
    Contributed,
    Inferred,
}

impl Origin {
    fn name(&self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Instrumented => "instrumented",
            Self::Contributed => "contributed",
            Self::Inferred => "inferred",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub id: String,
    pub subject: String,
    pub origin: Origin,
    pub value: Value,
    pub corrects: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    pub id: String,
    pub from: RecordRef,
    pub to: RecordRef,
    pub kind: String,
    pub origin: Origin,
    pub corrects: Option<String>,
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Forward,
    Reverse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextCursor {
    project: String,
    run: Reference,
    revision: u64,
    after: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextEntry {
    pub context: Option<Context>,
    pub value: ValueRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPage {
    pub revision: u64,
    pub contexts: Vec<ContextEntry>,
    pub continuation: Option<ContextCursor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationshipCursor {
    project: String,
    record: RecordRef,
    direction: Direction,
    revision: u64,
    after: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelationshipEntry {
    pub relationship: Option<Relationship>,
    pub from_present: bool,
    pub to_present: bool,
    pub value: ValueRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelationshipPage {
    pub revision: u64,
    pub relationships: Vec<RelationshipEntry>,
    pub continuation: Option<RelationshipCursor>,
}

impl Project {
    pub fn add_context(&self, run: &Reference, context: Context) -> Result<u64> {
        self.check_reference(run)?;
        project::validate_id(&context.id)?;
        project::validate_id(&context.subject)?;
        if let Some(corrects) = &context.corrects {
            project::validate_id(corrects)?;
        }
        let body = project::encode_bounded(&context)?;
        self.transaction(|transaction| {
            project::require_id(transaction,"runs",&run.id)?;
            let existing:Option<(String,u64)>=transaction.query_row("SELECT body,revision FROM contexts WHERE run=?1 AND id=?2",params![run.id,context.id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            if let Some((old,revision))=existing { return if old==body {Ok(revision)} else {Err(Error::Conflict(format!("context {}",context.id)))}; }
            if let Some(corrects)=&context.corrects
                && transaction.query_row("SELECT 1 FROM contexts WHERE run=?1 AND id=?2",params![run.id,corrects],|_|Ok(())).optional()?.is_none() {return Err(Error::NotFound(format!("context {corrects}")));}
            let revision=project::bump_revision(transaction)?;
            transaction.execute("INSERT INTO contexts(run,id,subject,origin,body,corrects,revision) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![run.id,context.id,context.subject,context.origin.name(),body,context.corrects,revision])?;
            Ok(revision)
        })
    }

    pub fn contexts(
        &self,
        run: &Reference,
        view: Option<u64>,
        limits: QueryLimits,
        cursor: Option<&ContextCursor>,
    ) -> Result<ContextPage> {
        self.check_reference(run)?;
        limits.validate()?;
        if let Some(cursor) = cursor {
            if cursor.project != self.id() {
                return Err(Error::ForeignProject);
            }
            if cursor.run != *run || view.is_some_and(|view| view != cursor.revision) {
                return Err(Error::Conflict("context continuation mismatch".into()));
            }
        }
        let budget = QueryControl::default().budget()?;
        let mut connection = self.reader(&budget)?;
        let transaction = connection.transaction()?;
        project::require_id(&transaction, "runs", &run.id)?;
        let revision = checked_view(&transaction, cursor.map(|cursor| cursor.revision).or(view))?;
        let mut after = cursor.map_or(0, |cursor| cursor.after);
        let token = ContextCursor {
            project: self.id().to_owned(),
            run: run.clone(),
            revision,
            after: i64::MAX,
        };
        let mut page = ContextPage {
            revision,
            contexts: Vec::new(),
            continuation: Some(token.clone()),
        };
        let mut used = encoded_size(&page)?;
        let mut statement=transaction.prepare("SELECT c.seq,c.id,length(CAST(c.body AS BLOB)) FROM contexts c WHERE c.run=?1 AND c.revision<=?2 AND c.seq>?3 AND NOT EXISTS(SELECT 1 FROM contexts n WHERE n.run=c.run AND n.corrects=c.id AND n.revision<=?2) ORDER BY c.seq LIMIT ?4")?;
        let mut rows = statement.query(params![run.id, revision, after, limits.max_rows + 1])?;
        let mut more = false;
        while let Some(row) = rows.next()? {
            if page.contexts.len() == limits.max_rows {
                more = true;
                break;
            }
            let seq: i64 = row.get(0)?;
            let id: String = row.get(1)?;
            let bytes: usize = row.get(2)?;
            let value = ValueRef {
                project: self.id().to_owned(),
                run: run.id.clone(),
                source: None,
                id,
                bytes,
                revision,
                row: seq,
                kind: StoredKind::Context,
            };
            let mut entry = ContextEntry {
                context: None,
                value,
            };
            let minimum = encoded_size(&entry)? + 1;
            if used + minimum > limits.max_bytes {
                if page.contexts.is_empty() {
                    return Err(Error::Limit("context reference exceeds page budget".into()));
                }
                more = true;
                break;
            }
            if used + minimum + bytes <= limits.max_bytes {
                let body: String = transaction.query_row(
                    "SELECT body FROM contexts WHERE seq=?1",
                    [seq],
                    |row| row.get(0),
                )?;
                entry.context = Some(serde_json::from_str(&body)?);
            }
            used += encoded_size(&entry)? + 1;
            after = seq;
            page.contexts.push(entry);
        }
        page.continuation = more.then_some(ContextCursor { after, ..token });
        if encoded_size(&page)? > limits.max_bytes {
            return Err(Error::Limit("context page exceeds budget".into()));
        }
        Ok(page)
    }

    pub fn add_relationship(&self, run: &Reference, relationship: Relationship) -> Result<u64> {
        self.add_relationships(run, std::slice::from_ref(&relationship))
    }

    pub fn add_relationships(
        &self,
        run: &Reference,
        relationships: &[Relationship],
    ) -> Result<u64> {
        self.check_reference(run)?;
        if relationships.len() > 256 {
            return Err(Error::Limit("relationship batch exceeds 256".into()));
        }
        let mut bodies = Vec::new();
        let mut bytes = 0_usize;
        for relationship in relationships {
            for id in [&relationship.id, &relationship.kind] {
                project::validate_id(id)?;
            }
            self.check_record_ref(&relationship.from)?;
            self.check_record_ref(&relationship.to)?;
            let body = project::encode_bounded(relationship)?;
            bytes += body.len();
            if bytes > 4 * 1024 * 1024 {
                return Err(Error::Limit("relationship batch exceeds 4 mib".into()));
            }
            bodies.push(body);
        }
        let budget = QueryControl::default().budget()?;
        self.transaction(|transaction| {
            project::require_id(transaction,"runs",&run.id)?;
            let mut revision=project::revision(transaction)?;let mut changed=false;
            for (relationship,body) in relationships.iter().zip(bodies) {
                budget.check()?;
                let existing:Option<String>=transaction.query_row("SELECT body FROM relationships WHERE run=?1 AND id=?2",params![run.id,relationship.id],|row|row.get(0)).optional()?;
                if let Some(existing)=existing {if existing==body {continue;}return Err(Error::Conflict(format!("relationship {}",relationship.id)));}
                for endpoint in [&relationship.from,&relationship.to] {project::require_id(transaction,"runs",&endpoint.run)?;}
                if let Some(corrects)=&relationship.corrects
                    && transaction.query_row("SELECT 1 FROM relationships WHERE run=?1 AND id=?2",params![run.id,corrects],|_|Ok(())).optional()?.is_none() {return Err(Error::NotFound(format!("relationship {corrects}")));}
                if !changed {revision=project::bump_revision(transaction)?;changed=true;}
                transaction.execute("INSERT INTO relationships(run,id,from_run,from_source,from_id,to_run,to_source,to_id,kind,origin,body,corrects,revision) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![run.id,relationship.id,relationship.from.run,relationship.from.source,relationship.from.id,relationship.to.run,relationship.to.source,relationship.to.id,relationship.kind,relationship.origin.name(),body,relationship.corrects,revision])?;
                if ["parent","part_of","causes"].contains(&relationship.kind.as_str()) && cyclic(transaction,relationship,&budget)? {return Err(Error::Invalid("causal or parent relationship closes a cycle".into()));}
            }
            Ok(revision)
        })
    }

    pub fn relationships(
        &self,
        record: &RecordRef,
        direction: Direction,
        view: Option<u64>,
        limits: QueryLimits,
        cursor: Option<&RelationshipCursor>,
    ) -> Result<RelationshipPage> {
        self.check_record_ref(record)?;
        limits.validate()?;
        if let Some(cursor) = cursor {
            if cursor.project != self.id() {
                return Err(Error::ForeignProject);
            }
            if cursor.record != *record
                || cursor.direction != direction
                || view.is_some_and(|view| view != cursor.revision)
            {
                return Err(Error::Conflict("relationship continuation mismatch".into()));
            }
        }
        let budget = QueryControl::default().budget()?;
        let mut connection = self.reader(&budget)?;
        let transaction = connection.transaction()?;
        let revision = checked_view(&transaction, cursor.map(|cursor| cursor.revision).or(view))?;
        let mut after = cursor.map_or(0, |cursor| cursor.after);
        let token = RelationshipCursor {
            project: self.id().to_owned(),
            record: record.clone(),
            direction,
            revision,
            after: i64::MAX,
        };
        let mut page = RelationshipPage {
            revision,
            relationships: Vec::new(),
            continuation: Some(token.clone()),
        };
        let mut used = encoded_size(&page)?;
        let endpoint = match direction {
            Direction::Forward => "from",
            Direction::Reverse => "to",
        };
        let sql = format!(
            "SELECT r.seq,r.run,r.id,length(CAST(r.body AS BLOB)),r.from_run,r.from_source,r.from_id,r.to_run,r.to_source,r.to_id FROM relationships r WHERE r.{endpoint}_run=?1 AND r.{endpoint}_source=?2 AND r.{endpoint}_id=?3 AND r.revision<=?4 AND r.seq>?5 AND NOT EXISTS(SELECT 1 FROM relationships n WHERE n.run=r.run AND n.corrects=r.id AND n.revision<=?4) ORDER BY r.seq LIMIT ?6"
        );
        let mut statement = transaction.prepare(&sql)?;
        let mut rows = statement.query(params![
            record.run,
            record.source,
            record.id,
            revision,
            after,
            limits.max_rows + 1
        ])?;
        let mut more = false;
        while let Some(row) = rows.next()? {
            if page.relationships.len() == limits.max_rows {
                more = true;
                break;
            }
            let seq: i64 = row.get(0)?;
            let run: String = row.get(1)?;
            let id: String = row.get(2)?;
            let bytes: usize = row.get(3)?;
            let from = RecordRef {
                project: self.id().to_owned(),
                run: row.get(4)?,
                source: row.get(5)?,
                id: row.get(6)?,
            };
            let to = RecordRef {
                project: self.id().to_owned(),
                run: row.get(7)?,
                source: row.get(8)?,
                id: row.get(9)?,
            };
            let value = ValueRef {
                project: self.id().to_owned(),
                run,
                id,
                source: None,
                bytes,
                revision,
                row: seq,
                kind: StoredKind::Relationship,
            };
            let mut entry = RelationshipEntry {
                relationship: None,
                from_present: present(&transaction, &from, revision)?,
                to_present: present(&transaction, &to, revision)?,
                value,
            };
            let minimum = encoded_size(&entry)? + 1;
            if used + minimum > limits.max_bytes {
                if page.relationships.is_empty() {
                    return Err(Error::Limit("relationship reference exceeds budget".into()));
                }
                more = true;
                break;
            }
            if used + minimum + bytes <= limits.max_bytes {
                let body: String = transaction.query_row(
                    "SELECT body FROM relationships WHERE seq=?1",
                    [seq],
                    |row| row.get(0),
                )?;
                entry.relationship = Some(serde_json::from_str(&body)?);
            }
            used += encoded_size(&entry)? + 1;
            after = seq;
            page.relationships.push(entry);
        }
        page.continuation = more.then_some(RelationshipCursor { after, ..token });
        if encoded_size(&page)? > limits.max_bytes {
            return Err(Error::Limit("relationship page exceeds budget".into()));
        }
        Ok(page)
    }
}

fn cyclic(
    connection: &Connection,
    relationship: &Relationship,
    budget: &QueryBudget,
) -> Result<bool> {
    use std::collections::{HashSet, VecDeque};
    let start = (
        relationship.to.run.clone(),
        relationship.to.source.clone(),
        relationship.to.id.clone(),
    );
    let target = (
        &relationship.from.run,
        &relationship.from.source,
        &relationship.from.id,
    );
    let mut visited = HashSet::from([start.clone()]);
    let mut pending = VecDeque::from([start]);
    let mut edges = 0;
    let mut statement=connection.prepare("SELECT r.to_run,r.to_source,r.to_id FROM relationships r WHERE r.from_run=?1 AND r.from_source=?2 AND r.from_id=?3 AND r.kind=?4 AND NOT EXISTS(SELECT 1 FROM relationships n WHERE n.run=r.run AND n.corrects=r.id) LIMIT 8193")?;
    while let Some((run, source, id)) = pending.pop_front() {
        budget.check()?;
        if (&run, &source, &id) == target {
            return Ok(true);
        }
        let mut rows = statement.query(params![run, source, id, relationship.kind])?;
        let mut fanout = 0;
        while let Some(row) = rows.next()? {
            budget.check()?;
            edges += 1;
            fanout += 1;
            if edges > 65536 || fanout > 8192 {
                return Err(Error::Limit(
                    "causal validation exceeds 65536 edges or 8192 outgoing edges".into(),
                ));
            }
            let next: (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
            if visited.insert(next.clone()) {
                pending.push_back(next);
            }
            if pending.len() > 8192 {
                return Err(Error::Limit(
                    "causal validation frontier exceeds 8192 identities".into(),
                ));
            }
        }
    }
    Ok(false)
}

fn present(connection: &Connection, reference: &RecordRef, revision: u64) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM records WHERE run=?1 AND source=?2 AND id=?3 AND revision<=?4",
            params![reference.run, reference.source, reference.id, revision],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
