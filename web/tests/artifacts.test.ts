import assert from "node:assert/strict";
import test from "node:test";
import {
  artifactReference,
  artifactSize,
  findArtifact,
  orderedArtifacts,
  type Artifact,
} from "../lib/kyler/artifacts.ts";

test("artifact references preserve filenames and exclude remote links", () => {
  assert.equal(
    artifactReference("./output/report%20one.pptx?download=1#slide"),
    "output/report one.pptx",
  );
  for (const value of [
    "https://example.com/file",
    "//example.com/file",
    "data:image/png,x",
    "#section",
    "?query",
    "%ZZ",
  ])
    assert.equal(artifactReference(value), null);
  assert.equal(artifactSize(1024), "1.0 KB");
  assert.equal(artifactSize(1048576), "1.0 MB");
});

test("artifact lookup requires the same session and execution", () => {
  const item = {
    session_id: "s1",
    execution_id: "e1",
    reference: "./output/report%20one.pptx",
  } as Artifact;
  assert.equal(findArtifact([item], "s1", "e1", "output/report one.pptx"), item);
  assert.equal(findArtifact([item], "s2", "e1", item.reference), undefined);
  assert.equal(findArtifact([item], "s1", "e2", item.reference), undefined);
  assert.equal(findArtifact([item], "s1", undefined, item.reference), undefined);
});

test("session outputs put recent conversation executions first and exclude other sessions", () => {
  const file = (id: string, execution_id: string, name: string, session_id = "s1") =>
    ({ id, execution_id, name, session_id }) as Artifact;
  const input = [
    file("old", "e1", "Report.pptx"),
    file("other", "e3", "Private.pdf", "s2"),
    file("new", "e2", "Report.pptx"),
    file("second", "e2", "Summary.pdf"),
  ];
  assert.deepEqual(
    orderedArtifacts(input, "s1", ["e1", "e2"]).map((item) => item.id),
    ["new", "second", "old"],
  );
  assert.equal(input[0].id, "old");
  assert.deepEqual(orderedArtifacts([], "s1", []), []);
});
