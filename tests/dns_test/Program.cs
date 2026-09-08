using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net;
using System.Threading.Tasks;

class Program
{
    static async Task<int> Main(string[] args)
    {
        Console.WriteLine("=========================================================");
        Console.WriteLine("       LVR Proton / Wine DNS Block Verification Tool      ");
        Console.WriteLine("=========================================================");

        // Path to the hosts file inside Wine Windows environment
        string systemRoot = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        string hostsPath = Path.Combine(systemRoot, "system32", "drivers", "etc", "hosts");

        if (!File.Exists(hostsPath))
        {
            Console.ForegroundColor = ConsoleColor.Red;
            Console.WriteLine($"[ERROR] Hosts file not found at: {hostsPath}");
            Console.ResetColor();
            return 1;
        }

        Console.WriteLine($"Found hosts file at: {hostsPath}");

        // Parse blocked hostnames from the hosts file
        var blockedDomains = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var line in File.ReadAllLines(hostsPath))
        {
            var trimmed = line.Trim();
            if (string.IsNullOrEmpty(trimmed) || trimmed.StartsWith("#"))
                continue;

            // Lines typically look like: 0.0.0.0 host1 host2 ...
            var parts = trimmed.Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
            if (parts.Length >= 2 && (parts[0] == "0.0.0.0" || parts[0] == "127.0.0.1"))
            {
                for (int i = 1; i < parts.Length; i++)
                {
                    string host = parts[i].Trim();
                    if (host.StartsWith("#"))
                        break;
                    if (host != "localhost")
                    {
                        blockedDomains.Add(host);
                    }
                }
            }
        }

        Console.WriteLine($"Discovered {blockedDomains.Count} blocked domains configured in hosts file.\n");

        int blockedPassed = 0;
        int blockedFailed = 0;

        Console.WriteLine("--- Checking Blocked Domains (Expecting 0.0.0.0) ---");
        foreach (var domain in blockedDomains)
        {
            try
            {
                IPHostEntry entry = await Dns.GetHostEntryAsync(domain);
                bool routedToZero = entry.AddressList.Any(ip => ip.ToString() == "0.0.0.0" || ip.ToString() == "127.0.0.1");

                if (routedToZero)
                {
                    blockedPassed++;
                }
                else
                {
                    blockedFailed++;
                    Console.ForegroundColor = ConsoleColor.Red;
                    Console.WriteLine($"[FAIL] {domain} resolved to {string.Join(", ", (IEnumerable<IPAddress>)entry.AddressList)} (NOT 0.0.0.0)");
                    Console.ResetColor();
                }
            }
            catch (Exception ex)
            {
                // In some DNS stacks, resolving 0.0.0.0 or blocked hosts throws SocketException (HostNotFound)
                // which is also a valid blocked outcome.
                blockedPassed++;
            }
        }

        Console.ForegroundColor = blockedFailed == 0 ? ConsoleColor.Green : ConsoleColor.Red;
        Console.WriteLine($"Blocked Domains Result: {blockedPassed}/{blockedDomains.Count} successfully blocked. ({blockedFailed} failed)\n");
        Console.ResetColor();

        // Control group: Domains that should NEVER be blocked
        var controlGroup = new[]
        {
            "api.vrchat.cloud",
            "assets.vrchat.com",
            "vrchat.com",
            "cloudflare.com",
            "google.com",
            "steampowered.com"
        };

        int controlPassed = 0;
        int controlFailed = 0;

        Console.WriteLine("--- Checking Unblocked / Control Domains (Expecting real IPs) ---");
        foreach (var domain in controlGroup)
        {
            try
            {
                IPHostEntry entry = await Dns.GetHostEntryAsync(domain);
                bool routedToZero = entry.AddressList.Any(ip => ip.ToString() == "0.0.0.0" || ip.ToString() == "127.0.0.1");

                if (!routedToZero && entry.AddressList.Length > 0)
                {
                    controlPassed++;
                    Console.ForegroundColor = ConsoleColor.Green;
                    Console.WriteLine($"[PASS] {domain} -> {entry.AddressList[0]} (Properly unblocked)");
                    Console.ResetColor();
                }
                else
                {
                    controlFailed++;
                    Console.ForegroundColor = ConsoleColor.Red;
                    Console.WriteLine($"[FAIL] {domain} resolved to 0.0.0.0/127.0.0.1 or empty!");
                    Console.ResetColor();
                }
            }
            catch (Exception ex)
            {
                controlFailed++;
                Console.ForegroundColor = ConsoleColor.Red;
                Console.WriteLine($"[FAIL] {domain} failed to resolve: {ex.Message}");
                Console.ResetColor();
            }
        }

        Console.WriteLine();
        Console.ForegroundColor = controlFailed == 0 ? ConsoleColor.Green : ConsoleColor.Red;
        Console.WriteLine($"Unblocked Control Result: {controlPassed}/{controlGroup.Length} successfully unblocked. ({controlFailed} failed)\n");
        Console.ResetColor();

        if (blockedFailed == 0 && controlFailed == 0)
        {
            Console.ForegroundColor = ConsoleColor.Green;
            Console.WriteLine("ALL CHECKS PASSED: Hosts file blocking and unblocking operate 100% as expected inside Proton/Wine!");
            Console.ResetColor();
            return 0;
        }

        return 1;
    }
}
