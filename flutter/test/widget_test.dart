import 'package:flutter_test/flutter_test.dart';
import 'package:mcp_hub/main.dart';

void main() {
  testWidgets('HubApp displays MCP Manager v3 title and import controls',
      (WidgetTester tester) async {
    await tester.pumpWidget(const HubApp());
    expect(find.text('MCP Manager v3'), findsOneWidget);
    expect(find.text('Ajout / Import Auto MCP'), findsOneWidget);
  });
}
