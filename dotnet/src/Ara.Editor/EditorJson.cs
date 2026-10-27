using System.Numerics;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Ara.Editor;

// Authoring values use the engine schema's arrays, enum names and address strings.
internal static class EditorJson
{
    static readonly JsonSerializerOptions Options = CreateOptions();
    internal static JsonElement Arguments<T>(T value) => JsonSerializer.SerializeToElement(value, Options);

    static JsonSerializerOptions CreateOptions()
    {
        var options = new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase, IncludeFields = true };
        options.Converters.Add(new JsonStringEnumConverter(JsonNamingPolicy.CamelCase));
        options.Converters.Add(new AddressConverter());
        options.Converters.Add(new NumberArray<Vector2>(v => [v.X, v.Y]));
        options.Converters.Add(new NumberArray<Vector3>(v => [v.X, v.Y, v.Z]));
        options.Converters.Add(new NumberArray<Vector4>(v => [v.X, v.Y, v.Z, v.W]));
        options.Converters.Add(new NumberArray<Quaternion>(v => [v.X, v.Y, v.Z, v.W]));
        return options;
    }

    sealed class AddressConverter : JsonConverter<EntityId>
    {
        public override EntityId Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options) => new(reader.GetString() ?? throw new JsonException("entity address must be text"));
        public override void Write(Utf8JsonWriter writer, EntityId value, JsonSerializerOptions options) => writer.WriteStringValue(value.Value);
    }

    sealed class NumberArray<T>(Func<T, float[]> values) : JsonConverter<T>
    {
        public override T Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options) => throw new NotSupportedException("authoring command values are write-only");
        public override void Write(Utf8JsonWriter writer, T value, JsonSerializerOptions options)
        {
            writer.WriteStartArray();
            foreach (var number in values(value)) writer.WriteNumberValue(number);
            writer.WriteEndArray();
        }
    }
}
