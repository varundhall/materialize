// Copyright Materialize, Inc. and contributors. All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use mz_ore::result::ResultExt;
use mz_repr::adt::date::Date;
use mz_repr::adt::numeric::Numeric;
use mz_repr::adt::timestamp::CheckedTimestamp;
use mz_repr::{Timestamp, strconv};

use crate::EvalError;

// Conversions to and from MzTimestamp to make it ergonomic to use. Although, theoretically, an
// MzTimestamp might not always mean milliseconds-since-unix-epoch, in practice it currently always
// does mean that. In order to increase usability of this type, we will provide casts and operators
// that make that assumption.

sqlfunc!(
    #[sqlname = "mz_timestamp_to_text"]
    #[preserves_uniqueness = true]
    #[inverse = to_unary!(super::CastStringToMzTimestamp)]
    fn cast_mz_timestamp_to_string(a: Timestamp) -> String {
        let mut buf = String::new();
        strconv::format_mz_timestamp(&mut buf, a);
        buf
    }
);

sqlfunc!(
    #[sqlname = "text_to_mz_timestamp"]
    #[preserves_uniqueness = false]
    #[inverse = to_unary!(super::CastMzTimestampToString)]
    fn cast_string_to_mz_timestamp(a: String) -> Result<Timestamp, EvalError> {
        strconv::parse_mz_timestamp(&a).err_into()
    }
);

sqlfunc!(
    #[sqlname = "numeric_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_numeric_to_mz_timestamp(a: Numeric) -> Result<Timestamp, EvalError> {
        // The try_into will error if the conversion is lossy (out of range or fractional).
        a.try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "uint8_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_uint64_to_mz_timestamp(a: u64) -> Timestamp {
        a.into()
    }
);

sqlfunc!(
    #[sqlname = "uint4_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_uint32_to_mz_timestamp(a: u32) -> Timestamp {
        u64::from(a).into()
    }
);

sqlfunc!(
    #[sqlname = "bigint_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_int64_to_mz_timestamp(a: i64) -> Result<Timestamp, EvalError> {
        a.try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "integer_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_int32_to_mz_timestamp(a: i32) -> Result<Timestamp, EvalError> {
        i64::from(a)
            .try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "timestamp_tz_to_mz_timestamp"]
    #[is_monotone = true]
    fn cast_timestamp_tz_to_mz_timestamp(
        a: CheckedTimestamp<DateTime<Utc>>,
    ) -> Result<Timestamp, EvalError> {
        a.timestamp_millis()
            .try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "timestamp_to_mz_timestamp"]
    #[is_monotone = true]
    fn cast_timestamp_to_mz_timestamp(
        a: CheckedTimestamp<NaiveDateTime>,
    ) -> Result<Timestamp, EvalError> {
        a.and_utc()
            .timestamp_millis()
            .try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "date_to_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn cast_date_to_mz_timestamp(a: Date) -> Result<Timestamp, EvalError> {
        let ts = CheckedTimestamp::try_from(NaiveDate::from(a).and_hms_opt(0, 0, 0).unwrap())?;
        ts.and_utc()
            .timestamp_millis()
            .try_into()
            .map_err(|_| EvalError::MzTimestampOutOfRange(a.to_string().into()))
    }
);

sqlfunc!(
    #[sqlname = "mz_timestamp_to_timestamp"]
    #[preserves_uniqueness = true]
    #[inverse = to_unary!(super::CastTimestampToMzTimestamp)]
    fn cast_mz_timestamp_to_timestamp(
        a: Timestamp,
    ) -> Result<CheckedTimestamp<NaiveDateTime>, EvalError> {
        let ms: i64 = a.try_into().map_err(|_| EvalError::TimestampOutOfRange)?;
        let ct = DateTime::from_timestamp_millis(ms).and_then(|dt| {
            let ct: Option<CheckedTimestamp<NaiveDateTime>> = dt.naive_utc().try_into().ok();
            ct
        });
        ct.ok_or(EvalError::TimestampOutOfRange)
    }
);

sqlfunc!(
    #[sqlname = "mz_timestamp_to_timestamp_tz"]
    #[preserves_uniqueness = true]
    #[inverse = to_unary!(super::CastTimestampTzToMzTimestamp)]
    fn cast_mz_timestamp_to_timestamp_tz(
        a: Timestamp,
    ) -> Result<CheckedTimestamp<DateTime<Utc>>, EvalError> {
        let ms: i64 = a.try_into().map_err(|_| EvalError::TimestampOutOfRange)?;
        let ct = DateTime::from_timestamp_millis(ms).and_then(|dt| {
            let ct: Option<CheckedTimestamp<DateTime<Utc>>> = dt.try_into().ok();
            ct
        });
        ct.ok_or(EvalError::TimestampOutOfRange)
    }
);

sqlfunc!(
    #[sqlname = "step_mz_timestamp"]
    #[preserves_uniqueness = true]
    #[is_monotone = true]
    fn step_mz_timestamp(a: Timestamp) -> Result<Timestamp, EvalError> {
        a.checked_add(1).ok_or(EvalError::MzTimestampStepOverflow)
    }
);
