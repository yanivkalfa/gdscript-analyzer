// Phase-1 exit demo (Node path): load the native binding, push a `.gd` file, and
// print its document symbols. Build the addon first with `npm run build`
// (requires @napi-rs/cli, which provisions libnode on Windows).
//
//   cd bindings/node && npm install && npm run build && npm run demo

// napi-rs emits a CommonJS `index.js`; load it from this ES module via createRequire.
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { AnalysisHandle } = require("./index.js");

const SRC = `class_name Player extends CharacterBody2D

const SPEED := 300.0
@export var health: int = 100
signal died(reason: String)
enum State { IDLE, RUNNING }

func _ready() -> void:
	var node := $Sprite2D
	print(node, health)

class Inner:
	var x = 1
	func helper() -> int:
		return x
`;

const handle = new AnalysisHandle();
handle.applyChange(0, SRC);

console.log("=== diagnostics ===");
console.log(handle.diagnostics(0)); // [] for valid source

console.log("\n=== document symbols ===");
const symbols = JSON.parse(handle.documentSymbols(0));
function print(sym, depth = 0) {
  console.log(`${"  ".repeat(depth)}${sym.kind.padEnd(9)} ${sym.name}  @${sym.range.start}..${sym.range.end}`);
  for (const child of sym.children) print(child, depth + 1);
}
for (const sym of symbols) print(sym);

console.log("\n=== completions at end of file ===");
const items = JSON.parse(handle.completions(0, SRC.length));
console.log(items.slice(0, 8).map((i) => `${i.kind}:${i.label}`).join(", "), "…");
