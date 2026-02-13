import sys
import json
import requests
from googlesearch import search
from bs4 import BeautifulSoup

def perform_search(query, num_results=3):
    results = []
    try:
        # Search Google
        urls = list(search(query, num_results=num_results, advanced=True))
        
        for result in urls:
            try:
                # Basic scrape
                headers = {'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36'}
                resp = requests.get(result.url, headers=headers, timeout=5)
                
                if resp.status_code == 200:
                    soup = BeautifulSoup(resp.content, 'html.parser')
                    
                    # Remove scripts and styles
                    for script in soup(["script", "style"]):
                        script.extract()
                        
                    # Get text
                    text = soup.get_text()
                    lines = (line.strip() for line in text.splitlines())
                    chunks = (phrase.strip() for line in lines for phrase in line.split("  "))
                    clean_text = ' '.join(chunk for chunk in chunks if chunk)[:500] # Limit context
                    
                    results.append({
                        "title": result.title,
                        "url": result.url,
                        "content": clean_text
                    })
            except Exception as e:
                pass
                
    except Exception as e:
        return {"error": str(e)}

    return results

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(json.dumps({"error": "No query provided"}))
        sys.exit(1)
        
    query = sys.argv[1]
    results = perform_search(query)
    print(json.dumps(results))
