using System;
using System.Diagnostics;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Threading.Tasks;

class Probe
{
    static async Task<int> Main(string[] args)
    {
        Console.WriteLine($"[PROBE] PID: {Process.GetCurrentProcess().Id}");
        Console.WriteLine($"[PROBE] OS: {Environment.OSVersion}");
        
        string[] targets = {
            "https://umbra999.github.io/VRCMapUtils/News.txt",
            "https://api.vrchat.cloud/api/1/config"
        };

        foreach (var url in targets)
        {
            var uri = new Uri(url);
            Console.WriteLine($"\n--- Target: {uri.Host} ---");
            try
            {
                var addresses = await Dns.GetHostAddressesAsync(uri.Host);
                Console.WriteLine($"[DNS] Resolved: {string.Join(", ", (object[])addresses)}");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[DNS] Error: {ex.Message}");
            }

            try
            {
                using var handler = new HttpClientHandler();
                using var client = new HttpClient(handler) { Timeout = TimeSpan.FromSeconds(5) };
                var resp = await client.GetAsync(uri);
                Console.WriteLine($"[HTTP] Status: {(int)resp.StatusCode} {resp.StatusCode}");
                if (resp.IsSuccessStatusCode)
                {
                    string snippet = await resp.Content.ReadAsStringAsync();
                    if (snippet.Length > 80) snippet = snippet[..80] + "...";
                    Console.WriteLine($"[HTTP] Body: {snippet.Replace("\n", " ").Replace("\r", "")}");
                }
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[HTTP] Error: {ex.Message}");
            }
        }
        return 0;
    }
}
