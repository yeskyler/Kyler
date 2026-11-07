pub mod answer;
pub mod dataset;
pub mod runner;
pub mod scorer;

pub const INSTRUCTIONS: &str = "Solve the supplied GAIA task using the available tools. Work independently; no human will answer questions or approvals. Do not search for benchmark answers or solutions. Put only the concise answer on a final line beginning FINAL ANSWER: . Do not include explanations or units after that marker unless the question asks for them.";
