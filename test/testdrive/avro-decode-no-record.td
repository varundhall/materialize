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
# Decoding a schema that is not a record results in a relation with a single column
#

$ set no-record
"int"

$ kafka-create-topic topic=avro-decode-no-record

$ kafka-ingest format=avro topic=avro-decode-no-record schema=${no-record} timestamp=1
123

> CREATE CONNECTION kafka_conn
  TO KAFKA (BROKER '${testdrive.kafka-addr}', SECURITY PROTOCOL PLAINTEXT);

> CREATE SOURCE avro_decode_no_record
  IN CLUSTER ${arg.single-replica-cluster}
  FROM KAFKA CONNECTION kafka_conn (TOPIC 'testdrive-avro-decode-no-record-${testdrive.seed}')

> CREATE TABLE avro_decode_no_record_tbl FROM SOURCE avro_decode_no_record (REFERENCE "testdrive-avro-decode-no-record-${testdrive.seed}")
  FORMAT AVRO USING SCHEMA '${no-record}'
  ENVELOPE NONE

> SHOW COLUMNS FROM avro_decode_no_record_tbl
name       nullable  type     comment
-------------------------------------
\?column?  false     integer  ""
