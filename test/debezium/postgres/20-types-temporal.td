# Copyright Materialize, Inc. and contributors. All rights reserved.
#
# Use of this software is governed by the Business Source License
# included in the LICENSE file at the root of this repository.
#
# As of the Change Date specified in that file, in accordance with
# the Business Source License, use of this software will be governed
# by the Apache License, Version 2.0.

#
# Make sure that temporal types are properly replicated, including sub-second precision
#

# TODO: Reenable when database-issues#2031 is fixed
$ skip-if
SELECT true

$ postgres-execute connection=postgres://postgres:postgres@postgres
CREATE TABLE temporal_types (date_col DATE, time_col TIME, timestamp_col TIMESTAMP);
ALTER TABLE temporal_types REPLICA IDENTITY FULL;
INSERT INTO temporal_types VALUES ('2011-11-11', '11:11:11.123456', '2011-11-11 11:11:11.123456');

> CREATE CONNECTION IF NOT EXISTS csr_conn TO CONFLUENT SCHEMA REGISTRY (
    URL '${testdrive.schema-registry-url}'
  );

> CREATE CONNECTION IF NOT EXISTS kafka_conn TO KAFKA (BROKER '${testdrive.kafka-addr}', SECURITY PROTOCOL PLAINTEXT);

> CREATE SOURCE temporal_types
  FROM KAFKA CONNECTION kafka_conn (TOPIC 'postgres.public.temporal_types');

> CREATE TABLE temporal_types_tbl FROM SOURCE temporal_types (REFERENCE "postgres.public.temporal_types")
  FORMAT AVRO USING CONFLUENT SCHEMA REGISTRY CONNECTION csr_conn
  ENVELOPE DEBEZIUM;

> SELECT * FROM temporal_types_tbl;
2011-11-11 11:11:11.123456 "2011-11-11 11:11:11.123456"

$ postgres-execute connection=postgres://postgres:postgres@postgres
UPDATE temporal_types SET date_col = '2012-12-12', time_col = '12:12:12.234567', timestamp_col = '2012-12-12 12:12:12.234567';

> SELECT * FROM temporal_types_tbl;
2012-12-12 12:12:12.234567 "2012-12-12 12:12:12.234567"
