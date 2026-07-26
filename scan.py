import ast
import json
import sys


path = sys.argv[1]
with open(path, "r") as f:
    tree = ast.parse(f.read())

for node in tree.body:
    if isinstance(node, ast.FunctionDef):
        print(node.name)