# JaCoCo Java fixture

This committed report is hand-authored; there is no JVM in the Rust-only CI
image. To regenerate against a local Java toolchain, initialize the project
with `phr-mcp init --packs java`, run
`phr-mcp coverage collect --tool java-cov --runner mvn --out cov`, then import
with `phr-mcp coverage import --format jacoco-dir --tool jacoco+mvn cov`.
The collection command writes one JaCoCo XML report per graph test and a
manifest stamped with the current Git revision and source digests.
