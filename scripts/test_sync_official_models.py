import importlib.util
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("sync_official_models.py")
SPEC = importlib.util.spec_from_file_location("sync_official_models", SCRIPT_PATH)
sync_official_models = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync_official_models)


class FindAdditionsTests(unittest.TestCase):
    def test_filters_hidden_new_slugs_and_preserves_existing_models(self):
        official = {
            "new-hidden-upstream-model": {
                "slug": "new-hidden-upstream-model",
                "visibility": "hide",
            },
            "new-visible-model": {
                "slug": "new-visible-model",
                "visibility": "list",
            },
            "existing-model": {
                "slug": "existing-model",
                "visibility": "hide",
            },
        }
        asset_models = [{"slug": "existing-model", "name": "Existing asset"}]
        original_asset_models = [dict(model) for model in asset_models]

        additions = sync_official_models.find_additions(official, asset_models)

        self.assertEqual(additions, ["new-visible-model"])
        self.assertEqual(asset_models, original_asset_models)


if __name__ == "__main__":
    unittest.main()
