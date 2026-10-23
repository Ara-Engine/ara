using System.Globalization;
using System.Text;

namespace Ara.Generators;

/// <summary>A minimal JSON writer for <c>types.json</c> fragments (no dependencies in the compiler).</summary>
public sealed class Json
{
    readonly StringBuilder _sb = new();
    bool _needComma;

    /// <summary>Starts an object (as a value or after <see cref="Key"/>).</summary>
    public Json Open() { Sep(); _sb.Append('{'); _needComma = false; return this; }

    /// <summary>Ends an object.</summary>
    public Json Close() { _sb.Append('}'); _needComma = true; return this; }

    /// <summary>Starts an array.</summary>
    public Json OpenArray() { Sep(); _sb.Append('['); _needComma = false; return this; }

    /// <summary>Ends an array.</summary>
    public Json CloseArray() { _sb.Append(']'); _needComma = true; return this; }

    /// <summary>Writes a key; the next call writes its value.</summary>
    public Json Key(string key) { Sep(); Str(key); _sb.Append(':'); _needComma = false; return this; }

    /// <summary>A string value.</summary>
    public Json String(string value) { Sep(); Str(value); _needComma = true; return this; }

    /// <summary>A number value.</summary>
    public Json Number(double value) { Sep(); _sb.Append(value.ToString("R", CultureInfo.InvariantCulture)); _needComma = true; return this; }

    /// <summary>A bool value.</summary>
    public Json Bool(bool value) { Sep(); _sb.Append(value ? "true" : "false"); _needComma = true; return this; }

    /// <summary>A null value.</summary>
    public Json Null() { Sep(); _sb.Append("null"); _needComma = true; return this; }

    /// <summary>Pre-rendered JSON.</summary>
    public Json Raw(string json) { Sep(); _sb.Append(json); _needComma = true; return this; }

    void Sep()
    {
        if (_needComma) _sb.Append(',');
        _needComma = false;
    }

    void Str(string s)
    {
        _sb.Append('"');
        foreach (var c in s)
        {
            switch (c)
            {
                case '"': _sb.Append("\\\""); break;
                case '\\': _sb.Append("\\\\"); break;
                case '\n': _sb.Append("\\n"); break;
                case '\r': _sb.Append("\\r"); break;
                case '\t': _sb.Append("\\t"); break;
                default:
                    if (c < 0x20) _sb.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                    else _sb.Append(c);
                    break;
            }
        }
        _sb.Append('"');
    }

    /// <inheritdoc/>
    public override string ToString() => _sb.ToString();
}
