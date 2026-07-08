/// Test-only utilities for apps built on `jeliya_protocol`: an in-memory
/// [MockClient] (ported from `ui/src/lib/mock.ts`) that answers every
/// PROTOCOL.md method from deterministic fixtures, for widget tests and demos.
///
/// Import as `package:jeliya_protocol/testing.dart` from tests/demos only —
/// deliberately NOT exported from the main `jeliya_protocol.dart` barrel so
/// production code never depends on fixtures.
library;

export 'testing/mock_client.dart';
