using Ara.Editor;

namespace Ara.EditorExamples;

#region topics/csharp-editor-api
/// <summary>Editor-only extension: menu and palette entries use the native registry.</summary>
public sealed class SceneTools : IEditorExtension
{
    /// <summary>Register one command and its menu placement for this compiled generation.</summary>
    public void Register(EditorRegistry registry)
    {
        registry.RegisterCommand("sample.organize", "Organize selection", context =>
        {
            var root = context.Scene.Find("Root");
            foreach (var selected in context.Selection.Entities)
            {
                _ = context.Scene.Parent(selected);
                _ = context.Scene.Children(selected);
                _ = context.Scene.Siblings(selected);
                context.Commands.SetProperty(selected, "name", "Organized");
                context.Commands.Reparent(selected, root?.Id);
                context.Commands.AddComponent(selected, "Transform", new { scale = System.Numerics.Vector3.One });
                context.Commands.RemoveComponent(selected, "Light");
            }
            var marker = context.Commands.Create("Marker", root?.Id);
            var temporary = context.Commands.Create("Temporary");
            context.Commands.Delete(temporary);
            context.Selection.Set([marker]);
        }, "Organize the selected entities through one attributed, undoable command batch.");
        registry.AddMenu("Tools/Organize selection", "sample.organize");
    }
}
#endregion
