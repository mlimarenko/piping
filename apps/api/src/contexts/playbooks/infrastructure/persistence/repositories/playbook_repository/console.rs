use super::CallbackTokenAuth;
use super::callback_tx::authorize_group_console_callback_tx;
use crate::errors::{AppError, classify_sqlx_error_owned};
use piping_contracts::playbook_runner::{
    ConsoleAck, ConsoleChunk, ConsolePage, ConsoleStream, MAX_CONSOLE_BATCH_BYTES,
    MAX_CONSOLE_BATCH_CHUNKS, RunnerConsoleBatch, validate_console_batch,
};
use sqlx::{PgPool, Postgres, Row};
use std::collections::HashMap;
use uuid::Uuid;

fn stream_label(stream: ConsoleStream) -> &'static str {
    match stream {
        ConsoleStream::Stdout => "stdout",
        ConsoleStream::Stderr => "stderr",
    }
}

fn parse_stream(value: &str) -> Result<ConsoleStream, AppError> {
    match value {
        "stdout" => Ok(ConsoleStream::Stdout),
        "stderr" => Ok(ConsoleStream::Stderr),
        other => Err(AppError::Database(format!(
            "Unknown console stream: {other}"
        ))),
    }
}

fn sequence_to_i64(sequence: u64) -> Result<i64, AppError> {
    i64::try_from(sequence)
        .map_err(|_| AppError::Validation("console sequence is out of range".to_string()))
}

fn page_next_sequence(chunks: &[ConsoleChunk], from_sequence: u64, durable_next: u64) -> u64 {
    match chunks.last() {
        Some(chunk) => chunk.sequence + 1,
        None => from_sequence.min(durable_next),
    }
}

fn page_is_complete(next_sequence: u64, sealed: Option<u64>) -> bool {
    sealed.is_some_and(|sealed| next_sequence >= sealed)
}

pub(super) async fn append_console_batch(
    pool: &PgPool,
    group_id: Uuid,
    batch: RunnerConsoleBatch,
    auth: CallbackTokenAuth,
) -> Result<ConsoleAck, AppError> {
    validate_console_batch(&batch).map_err(AppError::Validation)?;

    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    authorize_group_console_callback_tx(&mut tx, &auth, group_id).await?;

    let group_sql = r#"
            SELECT dispatch_id, dispatch_attempt, console_next_sequence, console_final_sequence
            FROM playbook_run_group
            WHERE id = $1
            FOR UPDATE
        "#;
    let group_row = sqlx::query(group_sql)
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?
        .ok_or_else(|| AppError::NotFound("Playbook run group not found".to_string()))?;
    let group_dispatch_id: Option<Uuid> = group_row
        .try_get("dispatch_id")
        .map_err(classify_sqlx_error_owned)?;
    let group_dispatch_attempt: i32 = group_row
        .try_get("dispatch_attempt")
        .map_err(classify_sqlx_error_owned)?;
    let next_sequence: i64 = group_row
        .try_get("console_next_sequence")
        .map_err(classify_sqlx_error_owned)?;
    let final_sequence: Option<i64> = group_row
        .try_get("console_final_sequence")
        .map_err(classify_sqlx_error_owned)?;

    if group_dispatch_id != Some(batch.dispatch_id)
        || group_dispatch_attempt != batch.dispatch_attempt
    {
        return Err(AppError::Conflict(
            "console dispatch identity does not match the durable run group".to_string(),
        ));
    }
    let next_sequence = u64::try_from(next_sequence)
        .map_err(|_| AppError::Database("durable console cursor is negative".to_string()))?;
    let sealed = final_sequence
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| AppError::Database("durable console seal is negative".to_string()))
        })
        .transpose()?;

    let first_sequence = batch
        .chunks
        .first()
        .map(|chunk| chunk.sequence)
        .ok_or_else(|| AppError::Validation("console batch is empty".to_string()))?;
    let last_sequence = batch
        .chunks
        .last()
        .map(|chunk| chunk.sequence)
        .unwrap_or(first_sequence);

    if let Some(sealed) = sealed {
        if last_sequence >= sealed {
            return Err(AppError::Conflict(
                "console stream is sealed; new chunks are rejected".to_string(),
            ));
        }
        verify_existing_console_chunks(&mut tx, group_id, batch.dispatch_id, &batch.chunks, sealed)
            .await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        return Ok(ConsoleAck {
            dispatch_id: batch.dispatch_id,
            dispatch_attempt: batch.dispatch_attempt,
            next_sequence,
        });
    }

    if first_sequence > next_sequence {
        return Err(AppError::Conflict(
            "console batch leaves a sequence gap".to_string(),
        ));
    }
    verify_existing_console_chunks(
        &mut tx,
        group_id,
        batch.dispatch_id,
        &batch.chunks,
        next_sequence,
    )
    .await?;

    let mut durable_next = next_sequence;
    for chunk in batch
        .chunks
        .iter()
        .filter(|chunk| chunk.sequence >= next_sequence)
    {
        let sequence = sequence_to_i64(chunk.sequence)?;
        sqlx::query(
            r#"
                INSERT INTO playbook_run_console_chunk (
                    run_group_id, dispatch_id, sequence, stream, bytes
                )
                VALUES ($1, $2, $3, $4, $5)
                "#,
        )
        .bind(group_id)
        .bind(batch.dispatch_id)
        .bind(sequence)
        .bind(stream_label(chunk.stream))
        .bind(chunk.bytes.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        durable_next = chunk.sequence + 1;
    }
    if durable_next > next_sequence {
        sqlx::query(
            r#"
                UPDATE playbook_run_group
                SET console_next_sequence = $2, updated_at = clock_timestamp()
                WHERE id = $1
                "#,
        )
        .bind(group_id)
        .bind(sequence_to_i64(durable_next)?)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(ConsoleAck {
        dispatch_id: batch.dispatch_id,
        dispatch_attempt: batch.dispatch_attempt,
        next_sequence: durable_next,
    })
}

pub(super) async fn list_console(
    pool: &PgPool,
    group_id: Uuid,
    from_sequence: u64,
    limit: i64,
    dispatch_id: Option<Uuid>,
) -> Result<ConsolePage, AppError> {
    let group_sql = r#"
            SELECT dispatch_id, dispatch_attempt, console_next_sequence, console_final_sequence
            FROM playbook_run_group
            WHERE id = $1
        "#;
    let group_row = sqlx::query(group_sql)
        .bind(group_id)
        .fetch_optional(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .ok_or_else(|| AppError::NotFound("Playbook run group not found".to_string()))?;
    let group_dispatch_id: Option<Uuid> = group_row
        .try_get("dispatch_id")
        .map_err(classify_sqlx_error_owned)?;
    let dispatch_attempt: i32 = group_row
        .try_get("dispatch_attempt")
        .map_err(classify_sqlx_error_owned)?;
    let durable_next: i64 = group_row
        .try_get("console_next_sequence")
        .map_err(classify_sqlx_error_owned)?;
    let final_sequence: Option<i64> = group_row
        .try_get("console_final_sequence")
        .map_err(classify_sqlx_error_owned)?;
    let durable_next = u64::try_from(durable_next)
        .map_err(|_| AppError::Database("durable console cursor is negative".to_string()))?;
    let sealed = final_sequence
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| AppError::Database("durable console seal is negative".to_string()))
        })
        .transpose()?;

    if let Some(expected) = dispatch_id
        && group_dispatch_id != Some(expected)
    {
        return Err(AppError::Conflict(
            "console dispatch identity does not match the durable run group".to_string(),
        ));
    }
    let Some(dispatch_id) = group_dispatch_id else {
        return Ok(ConsolePage {
            dispatch_id: None,
            dispatch_attempt: 0,
            chunks: Vec::new(),
            next_sequence: 0,
            complete: false,
        });
    };

    let limit = limit.clamp(1, MAX_CONSOLE_BATCH_CHUNKS as i64);
    let rows = sqlx::query(
        r#"
            SELECT sequence, stream, bytes
            FROM playbook_run_console_chunk
            WHERE run_group_id = $1
              AND dispatch_id = $2
              AND sequence >= $3
            ORDER BY sequence ASC
            LIMIT $4
            "#,
    )
    .bind(group_id)
    .bind(dispatch_id)
    .bind(sequence_to_i64(from_sequence)?)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(classify_sqlx_error_owned)?;

    let mut chunks = Vec::new();
    let mut byte_total = 0_usize;
    for row in rows {
        let sequence: i64 = row.try_get("sequence").map_err(classify_sqlx_error_owned)?;
        let stream: String = row.try_get("stream").map_err(classify_sqlx_error_owned)?;
        let bytes: Vec<u8> = row.try_get("bytes").map_err(classify_sqlx_error_owned)?;
        if !chunks.is_empty() && byte_total + bytes.len() > MAX_CONSOLE_BATCH_BYTES {
            break;
        }
        byte_total += bytes.len();
        chunks.push(ConsoleChunk {
            sequence: u64::try_from(sequence).map_err(|_| {
                AppError::Database("console chunk sequence is negative".to_string())
            })?,
            stream: parse_stream(&stream)?,
            bytes,
        });
    }

    let next_sequence = page_next_sequence(&chunks, from_sequence, durable_next);
    let complete = page_is_complete(next_sequence, sealed);
    Ok(ConsolePage {
        dispatch_id: Some(dispatch_id),
        dispatch_attempt,
        chunks,
        next_sequence,
        complete,
    })
}

async fn verify_existing_console_chunks(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    group_id: Uuid,
    dispatch_id: Uuid,
    chunks: &[ConsoleChunk],
    bound: u64,
) -> Result<(), AppError> {
    let sequences = chunks
        .iter()
        .filter(|chunk| chunk.sequence < bound)
        .map(|chunk| sequence_to_i64(chunk.sequence))
        .collect::<Result<Vec<_>, _>>()?;
    if sequences.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query(
        r#"
        SELECT sequence, stream, bytes
        FROM playbook_run_console_chunk
        WHERE run_group_id = $1
          AND dispatch_id = $2
          AND sequence = ANY($3)
        "#,
    )
    .bind(group_id)
    .bind(dispatch_id)
    .bind(sequences)
    .fetch_all(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let mut stored = HashMap::with_capacity(rows.len());
    for row in rows {
        let sequence: i64 = row.try_get("sequence").map_err(classify_sqlx_error_owned)?;
        let stream: String = row.try_get("stream").map_err(classify_sqlx_error_owned)?;
        let bytes: Vec<u8> = row.try_get("bytes").map_err(classify_sqlx_error_owned)?;
        stored.insert(sequence, (stream, bytes));
    }
    for chunk in chunks.iter().filter(|chunk| chunk.sequence < bound) {
        let sequence = sequence_to_i64(chunk.sequence)?;
        let Some((stream, bytes)) = stored.get(&sequence) else {
            return Err(AppError::Conflict(
                "console retry does not match the durable prefix".to_string(),
            ));
        };
        if stream.as_str() != stream_label(chunk.stream) || bytes != &chunk.bytes {
            return Err(AppError::Conflict(
                "console retry changed bytes or stream".to_string(),
            ));
        }
    }
    Ok(())
}

/// Seal the supervisor's final console prefix together with the terminal group
/// transition. Callback completion requires an exact durable count; server-side
/// failure completion may omit the seal.
pub(super) async fn seal_console_prefix_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    group_id: Uuid,
    dispatch_id: Uuid,
    dispatch_attempt: i32,
    console_next_sequence: u64,
) -> Result<bool, AppError> {
    let row = sqlx::query(
        r#"
        SELECT dispatch_id, dispatch_attempt, console_next_sequence, console_final_sequence,
               (status IN ('succeeded', 'failed', 'canceled', 'timed_out')
                AND finished_at IS NOT NULL) AS terminal
        FROM playbook_run_group
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(group_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .ok_or_else(|| AppError::NotFound("Playbook run group not found".to_string()))?;
    let terminal: bool = row.try_get("terminal").map_err(classify_sqlx_error_owned)?;
    let durable_dispatch_id: Option<Uuid> = row
        .try_get("dispatch_id")
        .map_err(classify_sqlx_error_owned)?;
    let durable_dispatch_attempt: i32 = row
        .try_get("dispatch_attempt")
        .map_err(classify_sqlx_error_owned)?;
    let durable_next: i64 = row
        .try_get("console_next_sequence")
        .map_err(classify_sqlx_error_owned)?;
    let final_sequence: Option<i64> = row
        .try_get("console_final_sequence")
        .map_err(classify_sqlx_error_owned)?;

    if durable_dispatch_id != Some(dispatch_id) || durable_dispatch_attempt != dispatch_attempt {
        return Err(AppError::Conflict(
            "completion dispatch identity does not match the durable run group".to_string(),
        ));
    }
    let requested = sequence_to_i64(console_next_sequence)?;
    if durable_next != requested {
        return Err(AppError::Conflict(
            "completion console count does not match the durable prefix".to_string(),
        ));
    }
    if let Some(final_sequence) = final_sequence {
        if final_sequence != requested {
            return Err(AppError::Conflict(
                "console stream is already sealed at a different prefix".to_string(),
            ));
        }
        return Ok(terminal);
    }
    sqlx::query(
        r#"
        UPDATE playbook_run_group
        SET console_final_sequence = $2, updated_at = clock_timestamp()
        WHERE id = $1
          AND console_final_sequence IS NULL
        "#,
    )
    .bind(group_id)
    .bind(requested)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(terminal)
}

#[cfg(test)]
mod tests {
    use super::{page_is_complete, page_next_sequence, parse_stream, stream_label};
    use piping_contracts::playbook_runner::{ConsoleChunk, ConsoleStream};

    fn chunk(sequence: u64) -> ConsoleChunk {
        ConsoleChunk {
            sequence,
            stream: ConsoleStream::Stdout,
            bytes: vec![b'x'],
        }
    }

    #[test]
    fn console_stream_labels_round_trip() {
        for stream in [ConsoleStream::Stdout, ConsoleStream::Stderr] {
            assert_eq!(parse_stream(stream_label(stream)).unwrap(), stream);
        }
        assert!(parse_stream("combined").is_err());
    }

    #[test]
    fn console_page_cursor_advances_only_past_returned_chunks() {
        assert_eq!(page_next_sequence(&[], 0, 0), 0);
        assert_eq!(page_next_sequence(&[], 3, 3), 3);
        assert_eq!(page_next_sequence(&[], 9, 3), 3);
        assert_eq!(page_next_sequence(&[chunk(2), chunk(3)], 2, 8), 4);
        assert_eq!(page_next_sequence(&[chunk(7)], 7, 8), 8);
    }

    #[test]
    fn console_page_complete_only_reaches_the_sealed_prefix() {
        assert!(!page_is_complete(0, None));
        assert!(!page_is_complete(4, Some(8)));
        assert!(page_is_complete(8, Some(8)));
        assert!(page_is_complete(9, Some(8)));
    }
}
