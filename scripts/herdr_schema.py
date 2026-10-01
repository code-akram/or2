#!/usr/bin/env python3
"""Schema helper for scripts/gen-herdr-types.sh. Not meant to be run by hand.

  herdr_schema.py normalize RAW_BUNDLE HERDR_VERSION OUT   normalized schema file
  herdr_schema.py split NORMALIZED OUTDIR                  one typify input per family

`herdr api schema --json` prints a bundle: `schemas.<family>` are JSON Schemas whose `$ref`s
point into the bundle (`#/schemas/<family>/$defs/X`), which typify cannot resolve. The
normalized file keeps the families that or2 generates code for with their refs rewritten to
local definitions (`#/$defs/X`), so each family stands alone, plus the herdr version and
protocol it came from.

Normalization also makes the client tolerant, because herdr may grow:
  * Validation keywords a client has no use for (`pattern`, `propertyNames`, `maxProperties`,
    `minProperties`) are dropped, so one odd map key cannot fail a whole snapshot.
  * In the families herdr *sends* (everything but `request`), a string enum becomes
    `oneOf [enum, string]`. typify turns that into an untagged enum whose second variant
    holds any value this build does not know, so a new status never fails a message.
  * `request` is split from its `id` envelope: the bundle describes `{id} + oneOf
    [{method, params}]`, which typify renders as an untyped struct. Only the `oneOf` is kept
    (titled `RequestBody`); typify generates an adjacently tagged enum (`method`, `params`)
    and or2's wire layer adds the `id`.

The `event` and `subscription_event` families are deliberately left out: typify renders their
internally tagged `EventData` as an enum that rejects an unknown event type, and nothing
consumes event payloads (events are invalidations). A consumer that needs them must add the
families here together with an open event-type tag.
"""

import copy
import json
import sys

FAMILIES = ["request", "success_response", "error_response"]
DROPPED_KEYWORDS = {"pattern", "propertyNames", "maxProperties", "minProperties"}
SENT_BY_CLIENT = {"request"}


def rewrite_refs(node, prefix):
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str):
            if not ref.startswith(prefix):
                raise SystemExit(f"unexpected $ref {ref!r}, expected prefix {prefix!r}")
            node["$ref"] = "#/" + ref[len(prefix):]
        for value in node.values():
            rewrite_refs(value, prefix)
    elif isinstance(node, list):
        for value in node:
            rewrite_refs(value, prefix)


def drop_validation(node):
    if isinstance(node, dict):
        for key in list(node):
            if key in DROPPED_KEYWORDS:
                del node[key]
            else:
                drop_validation(node[key])
    elif isinstance(node, list):
        for value in node:
            drop_validation(value)


def open_enums(node):
    """Rewrites every string enum to `oneOf [enum, string]`; returns how many."""
    count = 0
    if isinstance(node, dict):
        for value in list(node.values()):
            count += open_enums(value)
        if node.get("type") == "string" and isinstance(node.get("enum"), list):
            known = {"type": "string", "enum": node.pop("enum")}
            del node["type"]
            node["oneOf"] = [known, {"type": "string"}]
            count += 1
    elif isinstance(node, list):
        for value in node:
            count += open_enums(value)
    return count


def normalize(raw_path, herdr_version, out_path):
    bundle = json.load(open(raw_path))
    schemas = bundle["schemas"]
    missing = [name for name in FAMILIES if name not in schemas]
    if missing:
        raise SystemExit(f"bundle lacks schema families: {missing}")
    families = {}
    for name in FAMILIES:
        schema = copy.deepcopy(schemas[name])
        rewrite_refs(schema, f"#/schemas/{name}/")
        drop_validation(schema)
        if name == "request":
            for key in ("properties", "required", "type"):
                schema.pop(key, None)
            schema["title"] = "RequestBody"
        if name not in SENT_BY_CLIENT:
            open_enums(schema)
        families[name] = schema
    normalized = {
        "source": {
            "herdr_version": herdr_version,
            "protocol": bundle["protocol"],
            "schema_version": bundle["schema_version"],
            "command": "herdr api schema --json",
        },
        "families": families,
    }
    with open(out_path, "w") as out:
        json.dump(normalized, out, indent=1, sort_keys=True)
        out.write("\n")


def split(normalized_path, out_dir):
    normalized = json.load(open(normalized_path))
    for name, schema in normalized["families"].items():
        with open(f"{out_dir}/{name}.json", "w") as out:
            json.dump(schema, out, indent=1, sort_keys=True)
    print(normalized["source"]["herdr_version"], normalized["source"]["protocol"])


if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "normalize":
        normalize(*sys.argv[2:])
    elif len(sys.argv) == 4 and sys.argv[1] == "split":
        split(*sys.argv[2:])
    else:
        raise SystemExit(__doc__)
