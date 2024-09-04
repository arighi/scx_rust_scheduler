#!/usr/bin/env python
#
# Automatically generate a Linux scheduler in Rust via OpenAI.

import os
import sys
from openai import OpenAI

AI_PROMPT = """Modify the following scheduler to match these requirements:

%s
Keep all the original inclusions and Rust dependencies.
Keep all the original comments in the code.
Output the source code directly, never ever print any additional comment around the source code.

This is the original scheduler's source code:
%s"""

SCX_SRC_FILE = "src/main.rs"

def clean_file_content(file_path, output_path=None):
    try:
        # Read the content from the file
        with open(file_path, 'r') as file:
            content = file.read()

        # Find the delimiters
        start_delim = '```rust'
        end_delim = '```'

        # Find the start and end positions
        start_pos = content.find(start_delim)
        end_pos = content.find(end_delim, start_pos + len(start_delim))

        if start_pos != -1 and end_pos != -1:
            # Extract content between the delimiters
            start_pos += len(start_delim)
            extracted_content = content[start_pos:end_pos].strip()
        else:
            # No delimiters found; use the whole content
            extracted_content = content

        # Determine the output path
        if output_path is None:
            output_path = file_path

        # Write the cleaned content to the output file
        with open(output_path, 'w') as file:
            file.write(extracted_content)

    except FileNotFoundError:
        sys.stderr.write(f"ERROR: the file {file_path} does not exist.\n")
    except IOError as e:
        sys.stderr.write(f"ERROR: could not read/write file: {e}")

OPENAI_API_KEY = os.environ.get("OPENAI_API_KEY")
if OPENAI_API_KEY is None:
    sys.stderr.write("ERROR: env variable OPENAI_API_KEY not defined")
    sys.exit(1)

if len(sys.argv) < 2:
    sys.stderr.write("Usage: " + os.path.basename(sys.argv[0]) + " TEXT")
    sys.exit(1)

prompt = ""
with open(SCX_SRC_FILE) as fd:
    sched_src = fd.read()
    prompt = AI_PROMPT % (sys.argv[1], sched_src)

if True:
    client = OpenAI(
        api_key=os.environ.get("OPENAI_API_KEY"),
    )
    stream = client.chat.completions.create(
        model="gpt-4o",
        messages=[{"role": "user", "content": prompt}],
        stream=True,
    )
    with open(SCX_SRC_FILE, 'w') as file:
        for chunk in stream:
            line = chunk.choices[0].delta.content or ""
            sys.stdout.write(line)
            file.write(line)

    clean_file_content(SCX_SRC_FILE)
else:
    print(prompt)
    sys.exit(0)

os.system("cargo build --release")
os.system("sudo ./target/release/scx_rust_scheduler")
