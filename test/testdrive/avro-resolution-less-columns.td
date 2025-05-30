# Copyright Materialize, Inc. and contributors. All rights reserved.
#
# Use of this software is governed by the Business Source License
# included in the LICENSE file at the root of this repository.
#
# As of the Change Date specified in that file, in accordance with
# the Business Source License, use of this software will be governed
# by the Apache License, Version 2.0.

$ set-arg-default single-replica-cluster=quickstart

#
# Allow a writer schema that has a smaller number of columns if the reader schema has specified a default
#

$ set 2columns={"type": "record", "name": "schema_less_columns", "fields": [ {"name": "f1", "type": "string", "default": "default_f1"} , {"name": "f2", "type": "string", "default": "default_f2"}] }
$ set 1column={"type": "record", "name": "schema_less_columns", "fields": [ {"name": "f1", "type": "string"} ] }

$ kafka-create-topic topic=resolution-2to1

$ kafka-ingest format=avro topic=resolution-2to1 schema=${2columns} timestamp=1
{"f1": "val_f1a", "f2": "val_f2a"}

> CREATE CONNECTION IF NOT EXISTS csr_conn TO CONFLUENT SCHEMA REGISTRY (
    URL '${testdrive.schema-registry-url}'
  );

> CREATE CONNECTION kafka_conn
  TO KAFKA (BROKER '${testdrive.kafka-addr}', SECURITY PROTOCOL PLAINTEXT);

> CREATE SOURCE resolution_2to1
  IN CLUSTER ${arg.single-replica-cluster}
  FROM KAFKA CONNECTION kafka_conn (TOPIC 'testdrive-resolution-2to1-${testdrive.seed}')

> CREATE TABLE resolution_2to1_tbl FROM SOURCE resolution_2to1 (REFERENCE "testdrive-resolution-2to1-${testdrive.seed}")
  FORMAT AVRO USING CONFLUENT SCHEMA REGISTRY CONNECTION csr_conn
  ENVELOPE NONE

$ kafka-ingest format=avro topic=resolution-2to1 schema=${1column} timestamp=2
{"f1": "val_f1b"}

> SELECT * FROM resolution_2to1_tbl
f1 f2
---
val_f1a val_f2a
val_f1b default_f2
