use super::*;
use crate::records::SourceDefinition;
use rusqlite::{Row, Transaction};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CatalogScope {
    Investigations,
    Runs { investigation: String },
    Sources { run: String },
    Events { run: String, source: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogCursor {
    project: String,
    scope: CatalogScope,
    revision: u64,
    after: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogPage<T> {
    pub revision: u64,
    pub items: Vec<T>,
    pub continuation: Option<CatalogCursor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredValue<T> {
    pub value: Option<T>,
    pub reference: ValueRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvestigationInfo {
    pub reference: Reference,
    pub name: String,
    pub metadata: StoredValue<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunInfo {
    pub reference: Reference,
    pub investigation: Reference,
    pub conditions: StoredValue<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceInfo {
    pub run: Reference,
    pub id: String,
    pub ready: bool,
    pub closed: bool,
    pub cutoff: Option<u64>,
    pub gap_count: u64,
    pub definition: StoredValue<SourceDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceEventInfo {
    pub run: Reference,
    pub source: String,
    pub id: String,
    pub kind: String,
    pub revision: u64,
    pub metadata: StoredValue<Value>,
}

impl Project {
    pub fn investigations(
        &self,
        limits: QueryLimits,
        cursor: Option<&CatalogCursor>,
    ) -> Result<CatalogPage<InvestigationInfo>> {
        self.catalog(CatalogScope::Investigations, limits, cursor,
            "SELECT rowid,id,name,length(CAST(metadata AS BLOB)) FROM investigations WHERE created_revision<=?1 AND rowid>?2 ORDER BY rowid LIMIT ?3", Vec::new(),
            |transaction,row,revision,remaining| {
                let seq=row.get(0)?;let id:String=row.get(1)?;let bytes=row.get(3)?;
                let mut item=InvestigationInfo{reference:self.reference(&id),name:row.get(2)?,metadata:StoredValue{value:None,reference:self.catalog_value(StoredKind::Investigation,seq,("",None),id,bytes,revision)}};
                if encoded_size(&item)?+bytes<=remaining {item.metadata.value=Some(read_json(transaction,"investigations","metadata",seq)?);}
                if encoded_size(&item)?>remaining {item.metadata.value=None;}
                Ok(item)
            })
    }

    pub fn runs(
        &self,
        investigation: &Reference,
        limits: QueryLimits,
        cursor: Option<&CatalogCursor>,
    ) -> Result<CatalogPage<RunInfo>> {
        self.check_reference(investigation)?;
        self.catalog(CatalogScope::Runs{investigation:investigation.id.clone()},limits,cursor,
            "SELECT rowid,id,length(CAST(conditions AS BLOB)) FROM runs WHERE created_revision<=?1 AND rowid>?2 AND investigation=?4 ORDER BY rowid LIMIT ?3",vec![SqlValue::Text(investigation.id.clone())],
            |transaction,row,revision,remaining| {
                let seq=row.get(0)?;let id:String=row.get(1)?;let bytes=row.get(2)?;
                let mut item=RunInfo{reference:self.reference(&id),investigation:investigation.clone(),conditions:StoredValue{value:None,reference:self.catalog_value(StoredKind::Run,seq,(&id,None),id.clone(),bytes,revision)}};
                if encoded_size(&item)?+bytes<=remaining {item.conditions.value=Some(read_json(transaction,"runs","conditions",seq)?);}
                if encoded_size(&item)?>remaining {item.conditions.value=None;}
                Ok(item)
            })
    }

    pub fn sources(
        &self,
        run: &Reference,
        limits: QueryLimits,
        cursor: Option<&CatalogCursor>,
    ) -> Result<CatalogPage<SourceInfo>> {
        self.check_reference(run)?;
        self.catalog(CatalogScope::Sources{run:run.id.clone()},limits,cursor,
            "SELECT s.rowid,s.id,length(CAST(s.definition AS BLOB)),s.closed AND s.cutoff<=?1,CASE WHEN s.cutoff<=?1 THEN s.cutoff END,EXISTS(SELECT 1 FROM source_events e WHERE e.run=s.run AND e.source=s.id AND e.kind='ready' AND e.revision<=?1),(SELECT count(*) FROM source_events e WHERE e.run=s.run AND e.source=s.id AND e.kind='gap' AND e.revision<=?1) FROM sources s WHERE s.created_revision<=?1 AND s.rowid>?2 AND s.run=?4 ORDER BY s.rowid LIMIT ?3",vec![SqlValue::Text(run.id.clone())],
            |transaction,row,revision,remaining| {
                let seq=row.get(0)?;let id:String=row.get(1)?;let bytes=row.get(2)?;
                let mut item=SourceInfo{run:run.clone(),id:id.clone(),closed:row.get(3)?,cutoff:row.get(4)?,ready:row.get(5)?,gap_count:row.get(6)?,definition:StoredValue{value:None,reference:self.catalog_value(StoredKind::Source,seq,(&run.id,Some(id.clone())),id,bytes,revision)}};
                if encoded_size(&item)?+bytes<=remaining {item.definition.value=Some(read_json(transaction,"sources","definition",seq)?);}
                if encoded_size(&item)?>remaining {item.definition.value=None;}
                Ok(item)
            })
    }

    pub fn source_events(
        &self,
        run: &Reference,
        source: &str,
        limits: QueryLimits,
        cursor: Option<&CatalogCursor>,
    ) -> Result<CatalogPage<SourceEventInfo>> {
        self.check_reference(run)?;
        project::validate_id(source)?;
        self.catalog(CatalogScope::Events{run:run.id.clone(),source:source.to_owned()},limits,cursor,
            "SELECT seq,id,kind,revision,length(CAST(body AS BLOB)) FROM source_events WHERE revision<=?1 AND seq>?2 AND run=?4 AND source=?5 ORDER BY seq LIMIT ?3",vec![SqlValue::Text(run.id.clone()),SqlValue::Text(source.to_owned())],
            |transaction,row,revision,remaining| {
                let seq=row.get(0)?;let id:String=row.get(1)?;let bytes=row.get(4)?;
                let mut item=SourceEventInfo{run:run.clone(),source:source.to_owned(),id:id.clone(),kind:row.get(2)?,revision:row.get(3)?,metadata:StoredValue{value:None,reference:self.catalog_value(StoredKind::SourceEvent,seq,(&run.id,Some(source.to_owned())),id,bytes,revision)}};
                if encoded_size(&item)?+bytes<=remaining {item.metadata.value=Some(read_json(transaction,"source_events","body",seq)?);}
                if encoded_size(&item)?>remaining {item.metadata.value=None;}
                Ok(item)
            })
    }

    fn catalog_value(
        &self,
        kind: StoredKind,
        row: i64,
        scope: (&str, Option<String>),
        id: String,
        bytes: usize,
        revision: u64,
    ) -> ValueRef {
        ValueRef {
            project: self.id().to_owned(),
            run: scope.0.to_owned(),
            source: scope.1,
            id,
            bytes,
            revision,
            row,
            kind,
        }
    }

    fn catalog<T: Serialize>(
        &self,
        scope: CatalogScope,
        limits: QueryLimits,
        cursor: Option<&CatalogCursor>,
        sql: &str,
        extra: Vec<SqlValue>,
        mut decode: impl FnMut(&Transaction<'_>, &Row<'_>, u64, usize) -> Result<T>,
    ) -> Result<CatalogPage<T>> {
        limits.validate()?;
        if let Some(cursor) = cursor {
            if cursor.project != self.id() {
                return Err(Error::ForeignProject);
            }
            if cursor.scope != scope || cursor.after < 0 {
                return Err(Error::Conflict(
                    "catalog continuation belongs to another scope".into(),
                ));
            }
        }
        let budget = QueryControl::default().budget()?;
        (|| {
            let mut connection = self.reader(&budget)?;
            let transaction = connection.transaction()?;
            match &scope {
                CatalogScope::Investigations => {}
                CatalogScope::Runs { investigation } => {
                    project::require_id(&transaction, "investigations", investigation)?
                }
                CatalogScope::Sources { run } => project::require_id(&transaction, "runs", run)?,
                CatalogScope::Events { run, source } => {
                    if transaction
                        .query_row(
                            "SELECT 1 FROM sources WHERE run=?1 AND id=?2",
                            params![run, source],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_none()
                    {
                        return Err(Error::NotFound(format!("source {source}")));
                    }
                }
            }
            let revision = checked_view(&transaction, cursor.map(|cursor| cursor.revision))?;
            let mut after = cursor.map_or(0, |cursor| cursor.after);
            let token = CatalogCursor {
                project: self.id().to_owned(),
                scope,
                revision,
                after: i64::MAX,
            };
            let mut page = CatalogPage {
                revision,
                items: Vec::new(),
                continuation: Some(token.clone()),
            };
            let mut used = encoded_size(&page)?;
            if used > limits.max_bytes {
                return Err(Error::Limit(
                    "catalog continuation exceeds page budget".into(),
                ));
            }
            let mut args = vec![
                SqlValue::Integer(as_i64(revision)?),
                SqlValue::Integer(after),
                SqlValue::Integer((limits.max_rows + 1) as i64),
            ];
            args.extend(extra);
            let mut statement = transaction.prepare(sql)?;
            let mut rows = statement.query(params_from_iter(args))?;
            let mut more = false;
            while let Some(row) = rows.next()? {
                budget.check()?;
                if page.items.len() == limits.max_rows {
                    more = true;
                    break;
                }
                let item = decode(
                    &transaction,
                    row,
                    revision,
                    limits.max_bytes.saturating_sub(used + 1),
                )?;
                let cost = encoded_size(&item)? + 1;
                if used + cost > limits.max_bytes {
                    if page.items.is_empty() {
                        return Err(Error::Limit("catalog reference exceeds page budget".into()));
                    }
                    more = true;
                    break;
                }
                used += cost;
                after = row.get(0)?;
                page.items.push(item);
            }
            page.continuation = more.then_some(CatalogCursor { after, ..token });
            if encoded_size(&page)? > limits.max_bytes {
                return Err(Error::Limit("catalog result exceeds page budget".into()));
            }
            Ok(page)
        })()
        .map_err(query_error)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(
    transaction: &Transaction<'_>,
    table: &str,
    column: &str,
    row: i64,
) -> Result<T> {
    let body: String = transaction.query_row(
        &format!("SELECT {column} FROM {table} WHERE rowid=?1"),
        [row],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&body)?)
}
