# syntax=docker/dockerfile:1.27.1
# The read model's Kafka Connect worker: Apache Kafka's Connect runtime with Confluent's JDBC
# sink, Confluent's Protobuf converter and the PostgreSQL JDBC driver, each pinned by version and
# SHA-256. See RUNBOOK.md "Read Model".
FROM apache/kafka:4.3.2@sha256:852a0d49fc4e656b022e7f23785411802c9a088c78a7c5e39f33969de9c227a7

ARG HUB=https://hub-downloads.confluent.io/api/plugins/confluentinc
ARG JDBC_SINK=10.9.9
ARG PROTOBUF_CONVERTER=8.3.2
ARG PGJDBC=42.7.14

ADD --checksum=sha256:6b9b479b2c99f7dafa1053d4a3ffa19d7421d4322ead7071cabee1c55f923308 \
    ${HUB}/kafka-connect-jdbc/versions/${JDBC_SINK}/confluentinc-kafka-connect-jdbc-${JDBC_SINK}.zip /tmp/
ADD --checksum=sha256:5617917614455c6af7f28bbf001daf0b24e1d2f24141ff9183db89540450cb4b \
    ${HUB}/kafka-connect-protobuf-converter/versions/${PROTOBUF_CONVERTER}/confluentinc-kafka-connect-protobuf-converter-${PROTOBUF_CONVERTER}.zip /tmp/
ADD --chmod=644 --checksum=sha256:73914527305a40cce504b0d3d90b23caf565136912d607ae8ae7c5895512332c \
    https://repo1.maven.org/maven2/org/postgresql/postgresql/${PGJDBC}/postgresql-${PGJDBC}.jar /tmp/

# One directory per plugin under plugin.path. The JDBC sink's bundled PostgreSQL driver is
# replaced with the pinned one.
USER root
RUN mkdir -p /opt/vex-connect/plugins \
    && unzip -q /tmp/confluentinc-kafka-connect-jdbc-${JDBC_SINK}.zip -d /opt/vex-connect/plugins \
    && unzip -q /tmp/confluentinc-kafka-connect-protobuf-converter-${PROTOBUF_CONVERTER}.zip -d /opt/vex-connect/plugins \
    && rm /opt/vex-connect/plugins/confluentinc-kafka-connect-jdbc-${JDBC_SINK}/lib/postgresql-*.jar \
    && mv /tmp/postgresql-${PGJDBC}.jar /opt/vex-connect/plugins/confluentinc-kafka-connect-jdbc-${JDBC_SINK}/lib/ \
    && rm /tmp/*.zip
USER appuser

COPY worker.properties /opt/vex-connect/worker.properties

EXPOSE 8083
CMD ["/opt/kafka/bin/connect-distributed.sh", "/opt/vex-connect/worker.properties"]
