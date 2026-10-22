// Headless integration smoke only. No font engine means this is not a text/performance comparison.
#include <RmlUi/Core.h>
#include <iostream>

class Renderer final : public Rml::RenderInterface {
public:
    int draws = 0;
    Rml::CompiledGeometryHandle next = 1;
    Rml::CompiledGeometryHandle CompileGeometry(Rml::Span<const Rml::Vertex>, Rml::Span<const int>) override { return next++; }
    void RenderGeometry(Rml::CompiledGeometryHandle, Rml::Vector2f, Rml::TextureHandle) override { ++draws; }
    void ReleaseGeometry(Rml::CompiledGeometryHandle) override {}
    Rml::TextureHandle LoadTexture(Rml::Vector2i&, const Rml::String&) override { return 0; }
    Rml::TextureHandle GenerateTexture(Rml::Span<const Rml::byte>, Rml::Vector2i) override { return 0; }
    void ReleaseTexture(Rml::TextureHandle) override {}
    void EnableScissorRegion(bool) override {}
    void SetScissorRegion(Rml::Rectanglei) override {}
};
class System final : public Rml::SystemInterface {
public:
    bool LogMessage(Rml::Log::Type, const Rml::String& message) override { std::cerr << message << '\n'; return true; }
};
int main() {
    Renderer renderer;
    System system;
    Rml::FontEngineInterface fonts;
    Rml::SetRenderInterface(&renderer);
    Rml::SetSystemInterface(&system);
    Rml::SetFontEngineInterface(&fonts);
    if (!Rml::Initialise()) return 1;
    auto* context = Rml::CreateContext("s3", {640, 480});
    if (!context) return 2;
    int health = 86;
    auto model = context->CreateDataModel("spike");
    if (!model || !model.Bind("health", &health)) return 3;
    auto handle = model.GetModelHandle();
    auto* document = context->LoadDocumentFromMemory(R"(
        <rml><head><style>
        body { width:640px; height:480px; margin:0; display:flex; flex-direction:column; padding:20px; }
        #health { width:300px; height:80px; background-color:#236d63; }
        #control { width:300px; height:60px; background-color:#243448; }
        #control:hover { background-color:#34546a; }
        </style></head><body data-model="spike"><!-- keep-source-comment --><div id="health" data-attr-value="health"/><button id="control"/></body></rml>
    )");
    if (!document) return 4;
    document->Show();
    context->Update();
    context->Render();
    auto* element = document->GetElementById("health");
    if (!element || element->GetAttribute<int>("value", 0) != 86) return 5;
    health = 87;
    handle.DirtyVariable("health");
    context->ProcessMouseMove(40, 125, 0);
    context->Update();
    context->Render();
    const bool binding = element->GetAttribute<int>("value", 0) == 87;
    const bool grid = element->SetProperty("display", "grid");
    const bool comment = document->GetInnerRML().find("keep-source-comment") != Rml::String::npos;
    std::cout << "{\"initialized\":true,\"document_loaded\":true,\"binding_updated\":" << (binding ? "true" : "false")
              << ",\"grid_display_accepted\":" << (grid ? "true" : "false") << ",\"geometry_draw_calls\":" << renderer.draws
              << ",\"runtime_serialization_preserves_comment\":" << (comment ? "true" : "false")
              << ",\"font_engine\":\"none; no text quality or timing claim\"}\n";
    document->Close();
    Rml::Shutdown();
    return binding && renderer.draws > 0 && !grid ? 0 : 6;
}
