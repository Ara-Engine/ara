using Ara.Examples;

if (args is ["--trace-commands"])
    Console.WriteLine((await CommandExamples.TraceAsync()).ToJsonString());
else
    throw new ArgumentException("Use --trace-commands to validate the compiled authoring recipes.");
