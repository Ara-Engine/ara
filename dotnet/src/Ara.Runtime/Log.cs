using Ara.Hosting;

namespace Ara;

/// <summary>Logging to the host console and log.</summary>
public static class Log
{
    /// <summary>Info-level message.</summary>
    public static void Info(string message) => Host.Log(0, message);

    /// <summary>Warning.</summary>
    public static void Warn(string message) => Host.Log(1, message);

    /// <summary>Error.</summary>
    public static void Error(string message) => Host.Log(2, message);
}
