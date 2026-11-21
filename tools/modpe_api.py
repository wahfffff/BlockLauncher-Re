#!/usr/bin/env python3
"""modpe_api.py - read the ModPE API surface out of a BlockLauncher APK.

This reads `classes.dex` and reports **names and signatures only**: class names,
method names, parameter types, return types and the access flags. It never
touches method bodies, and it emits no bytecode, no string constants and no
resource content.

That distinction is the point. A JavaScript API is a list of names and calling
conventions; to run a third-party script written against it, a launcher has to
know those names. Knowing them is interoperability. Copying the implementation
would be a different thing, and this tool cannot do it — the output is a table.

The table is also useful as a historical record: it is the exact surface the
last BlockLauncher shipped, so a script's report of "X is undefined" can be
checked against what X actually was, instead of against memory.

Usage:
    python3 tools/modpe_api.py <BlockLauncher.apk> [--json out.json] [--class NAME]

Without --class it summarises every class under the modpe packages.
"""

import argparse
import json
import re
import struct
import sys
import zipfile

DEX_MAGIC = b"dex\n"


class Reader:
    def __init__(self, blob):
        self.b = blob

    def u16(self, off):
        return struct.unpack_from("<H", self.b, off)[0]

    def u32(self, off):
        return struct.unpack_from("<I", self.b, off)[0]

    def uleb(self, off):
        result = 0
        shift = 0
        while True:
            byte = self.b[off]
            off += 1
            result |= (byte & 0x7F) << shift
            if byte < 0x80:
                return result, off
            shift += 7
            if shift > 35:
                raise ValueError("uleb128 too long")


def decode_mutf8(raw):
    # MUTF-8 differs from UTF-8 only in NUL and surrogate encoding; names are
    # ASCII in practice, and a lossy decode keeps a weird name readable instead
    # of aborting the whole table.
    return raw.split(b"\x00", 1)[0].decode("utf-8", "replace")


class Dex:
    def __init__(self, blob):
        if blob[:4] != DEX_MAGIC:
            raise ValueError("not a dex file")
        r = Reader(blob)
        self.r = r
        self.string_ids_off = r.u32(0x3C)
        self.string_ids_size = r.u32(0x38)
        self.type_ids_off = r.u32(0x44)
        self.type_ids_size = r.u32(0x40)
        self.proto_ids_off = r.u32(0x4C)
        self.proto_ids_size = r.u32(0x48)
        self.method_ids_off = r.u32(0x5C)
        self.method_ids_size = r.u32(0x58)
        self.class_defs_off = r.u32(0x64)
        self.class_defs_size = r.u32(0x60)

    def string(self, idx):
        data_off = self.r.u32(self.string_ids_off + idx * 4)
        _, p = self.r.uleb(data_off)
        end = self.r.b.index(b"\x00", p)
        return decode_mutf8(self.r.b[p:end])

    def type_name(self, idx):
        return self.string(self.r.u32(self.type_ids_off + idx * 4))

    def proto(self, idx):
        base = self.proto_ids_off + idx * 12
        ret_idx = self.r.u32(base + 4)
        params_off = self.r.u32(base + 8)
        params = []
        if params_off:
            n = self.r.u32(params_off)
            for i in range(n):
                params.append(self.type_name(self.r.u16(params_off + 4 + i * 2)))
        return self.type_name(ret_idx), params

    def method(self, idx):
        base = self.method_ids_off + idx * 8
        class_idx = self.r.u16(base)
        proto_idx = self.r.u16(base + 2)
        name_idx = self.r.u32(base + 4)
        ret, params = self.proto(proto_idx)
        return self.type_name(class_idx), self.string(name_idx), ret, params

    def classes(self):
        for i in range(self.class_defs_size):
            base = self.class_defs_off + i * 32
            name = self.type_name(self.r.u32(base))
            data_off = self.r.u32(base + 24)
            yield name, data_off

    def class_methods(self, data_off):
        """Yield (name, ret, params, is_static, is_public) for a class_data_item."""
        if not data_off:
            return
        p = data_off
        r = self.r
        static_fields, p = r.uleb(p)
        instance_fields, p = r.uleb(p)
        direct_methods, p = r.uleb(p)
        virtual_methods, p = r.uleb(p)

        for _ in range(static_fields):
            _, p = r.uleb(p)
            _, p = r.uleb(p)
        for _ in range(instance_fields):
            _, p = r.uleb(p)
            _, p = r.uleb(p)

        for count, is_direct in ((direct_methods, True), (virtual_methods, False)):
            idx = 0
            for _ in range(count):
                diff, p = r.uleb(p)
                access, p = r.uleb(p)
                _, p = r.uleb(p)  # code_off: read, never followed
                idx += diff
                _, name, ret, params = self.method(idx)
                is_static = bool(access & 0x0008) and is_direct
                is_public = bool(access & 0x0001)
                yield name, ret, params, is_static, is_public


INTERESTING = re.compile(r"mcpelauncher/(api/modpe/|ScriptManager)")


def load_dex(apk_path):
    with zipfile.ZipFile(apk_path) as z:
        names = [n for n in z.namelist() if re.fullmatch(r"classes\d*\.dex", n)]
        if not names:
            raise SystemExit("no classes.dex in the archive")
        # Only the first dex is read: classes added later are the launcher's own
        # UI, and the API surface has always been in the primary dex.
        return Dex(z.read(sorted(names)[0]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("apk")
    ap.add_argument("--json")
    ap.add_argument("--class", dest="only")
    ap.add_argument("--all", action="store_true", help="include non-public methods")
    args = ap.parse_args()

    dex = load_dex(args.apk)
    table = {}

    for cls, data_off in dex.classes():
        if not INTERESTING.search(cls):
            continue
        simple = cls.strip("L").rstrip(";").replace("/", ".")
        if args.only and args.only.lower() not in simple.lower():
            continue
        methods = []
        for name, ret, params, is_static, is_public in dex.class_methods(data_off):
            if not args.all and not is_public:
                continue
            methods.append(
                {
                    "name": name,
                    "returns": ret,
                    "params": params,
                    "static": is_static,
                }
            )
        if methods:
            table[simple] = sorted(methods, key=lambda m: m["name"])

    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(table, f, indent=2, ensure_ascii=False)
        print(f"wrote {args.json}")

    total = sum(len(v) for v in table.values())
    print(f"{len(table)} classes, {total} public methods\n")
    for cls, methods in sorted(table.items(), key=lambda kv: -len(kv[1])):
        print(f"{cls}  ({len(methods)})")
        if args.only or len(methods) <= 40:
            for m in methods:
                sig = ",".join(m["params"])
                print(f"    {'static ' if m['static'] else '       '}{m['returns']} {m['name']}({sig})")
        else:
            names = ", ".join(m["name"] for m in methods[:12])
            print(f"    {names}, ...")


if __name__ == "__main__":
    sys.exit(main())
