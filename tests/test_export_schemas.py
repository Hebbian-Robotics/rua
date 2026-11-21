import unittest
from pathlib import Path

import pytest

pytestmark = pytest.mark.contracts


class ViewerSchemaTests(unittest.TestCase):
    def test_committed_schemas_match_the_models(self) -> None:
        from rua_dev.viewer_schemas import VIEWER_SCHEMAS, schema_text

        for schema in VIEWER_SCHEMAS:
            with self.subTest(schema=schema.name):
                self.assertEqual(
                    (
                        Path(__file__).resolve().parents[1] / schema.schema_path
                    ).read_text(),
                    schema_text(schema.adapter),
                    f"The {schema.name} model changed; run `uv run rua-dev viewer-schemas`",
                )
