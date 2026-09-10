using System.Net;

// Read-only diagnostic for a selected Wine/Windows hosts file. Public controls require DNS.
string hostsPath = args.Length > 0 ? args[0] : Path.Combine(
    Environment.GetFolderPath(Environment.SpecialFolder.Windows), "system32", "drivers", "etc", "hosts");
if (!File.Exists(hostsPath))
{
    Console.Error.WriteLine($"Hosts file not found: {hostsPath}");
    return 2;
}
var domains = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
foreach (string line in File.ReadLines(hostsPath))
{
    string[] fields = line.Split('#', 2)[0].Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
    if (fields.Length < 2 || (fields[0] != "0.0.0.0" && fields[0] != "::")) continue;
    foreach (string field in fields.Skip(1))
    {
        // Probe a representative name for simple wildcard patterns.
        string host = field.StartsWith("*.", StringComparison.Ordinal) ? "lvr-dns-probe." + field[2..] : field;
        if (host.Contains('*') || host.Contains('?'))
        {
            Console.WriteLine($"SKIP unsupported diagnostic pattern: {field}");
            continue;
        }
        domains.Add(host);
    }
}
if (domains.Count == 0)
{
    Console.Error.WriteLine("No testable blocking rules were found; no verification performed.");
    return 2;
}
int failures = 0;
foreach (string host in domains.Order())
{
    try
    {
        IPAddress[] addresses = await Dns.GetHostAddressesAsync(host).WaitAsync(TimeSpan.FromSeconds(5));
        bool blocked = addresses.Length > 0 && addresses.All(ip => ip.Equals(IPAddress.Any) || ip.Equals(IPAddress.IPv6Any));
        if (!blocked) failures++;
        Console.WriteLine($"{(blocked ? "PASS" : "FAIL")} {host}: {string.Join(", ", addresses.Select(ip => ip.ToString()))}");
    }
    catch (Exception error) when (error is System.Net.Sockets.SocketException or TimeoutException)
    {
        // A DNS failure does not prove that our interceptor blocked the domain.
        failures++;
        Console.Error.WriteLine($"INCONCLUSIVE {host}: {error.Message}");
    }
}
foreach (string host in new[] { "api.vrchat.cloud", "assets.vrchat.com" })
{
    try
    {
        IPAddress[] addresses = await Dns.GetHostAddressesAsync(host).WaitAsync(TimeSpan.FromSeconds(5));
        bool allowed = addresses.Length > 0 && addresses.All(ip => !ip.Equals(IPAddress.Any) && !ip.Equals(IPAddress.IPv6Any) && !IPAddress.IsLoopback(ip));
        if (!allowed) failures++;
        Console.WriteLine($"{(allowed ? "PASS" : "FAIL")} control: {host}");
    }
    catch (Exception error) when (error is System.Net.Sockets.SocketException or TimeoutException)
    {
        failures++;
        Console.Error.WriteLine($"FAIL control {host}: {error.Message}");
    }
}
Console.WriteLine($"Checked {domains.Count} mapped hostnames and 2 controls; {failures} failed/inconclusive.");
return failures == 0 ? 0 : 1;
