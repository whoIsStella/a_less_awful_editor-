// Native adapter for the pinned Apache-2.0 Ghidra decompiler (see README.md).
// Input/output are private XML between worker.py and this process. No target runs.
#include "libdecomp.hh"
#include "xml_arch.hh"
#include <memory>

using namespace ghidra;

static uintb number(const string &value) {
  size_t end = 0;
  uintb result = std::stoull(value, &end, 0);
  if (end != value.size()) throw LowlevelError("Invalid address");
  return result;
}

static string escaped(const string &value) {
  string result;
  for (char c : value) {
    switch (c) {
    case '&': result += "&amp;"; break;
    case '<': result += "&lt;"; break;
    case '>': result += "&gt;"; break;
    case '"': result += "&quot;"; break;
    default: result += c;
    }
  }
  return result;
}

static Datatype *type(Architecture &architecture, const string &name) {
  if (name == "void") return architecture.types->getTypeVoid();
  if (name == "bool") return architecture.types->getBase(1, TYPE_BOOL);
  for (int size : {1, 2, 4, 8}) {
    if (name == "int" + std::to_string(size * 8))
      return architecture.types->getBase(size, TYPE_INT);
    if (name == "uint" + std::to_string(size * 8))
      return architecture.types->getBase(size, TYPE_UINT);
  }
  throw LowlevelError("Unsupported metadata type: " + name);
}

int main(int argc, char **argv) {
  if (argc != 2) {
    std::cerr << "Expected one SLEIGH language directory argument\n";
    return 2;
  }
  try {
    startDecompilerLibrary(vector<string>{argv[1]});
    DocumentStorage storage;
    const Element *request = storage.parseDocument(std::cin)->getRoot();
    if (request->getName() != "request") throw LowlevelError("Invalid request root");
    const Element *image = nullptr;
    vector<const Element *> functions;
    for (const Element *child : request->getChildren()) {
      if (child->getName() == "binaryimage") image = child;
      else if (child->getName() == "function") functions.push_back(child);
      else throw LowlevelError("Unknown request element");
    }
    if (!image) throw LowlevelError("Missing mapped image");
    storage.registerTag(image);
    std::ostringstream diagnostics;
    XmlArchitecture architecture("snapshot", "", &diagnostics);
    architecture.init(storage);
    architecture.max_instructions = 10000;
    AddrSpace *ram = architecture.getDefaultCodeSpace();
    Scope *global = architecture.symboltab->getGlobalScope();
    for (const Element *metadata : functions) {
      Address address(ram, number(metadata->getAttributeValue("address")));
      Funcdata *function = global->addFunction(address, metadata->getAttributeValue("name"))->getFunction();
      if (metadata->getAttributeValue("typed") != "true") continue;
      PrototypePieces prototype;
      prototype.model = architecture.defaultfp;
      prototype.name = metadata->getAttributeValue("name");
      prototype.outtype = type(architecture, metadata->getAttributeValue("return_type"));
      prototype.firstVarArgSlot = -1;
      for (const Element *parameter : metadata->getChildren()) {
        prototype.intypes.push_back(type(architecture, parameter->getAttributeValue("type")));
        prototype.innames.push_back(parameter->getAttributeValue("name"));
      }
      function->getFuncProto().setPieces(prototype);
      function->getFuncProto().setInputLock(true);
      function->getFuncProto().setOutputLock(true);
    }
    Address entry(ram, number(request->getAttributeValue("entry")));
    Funcdata *function = global->queryFunction(entry);
    if (!function)
      function = global->addFunction(entry, request->getAttributeValue("name"))->getFunction();
    function->followFlow(Address(ram, 0), Address(ram, ram->getHighest()));
    architecture.allacts.getCurrent()->reset(*function);
    if (architecture.allacts.getCurrent()->perform(*function) < 0)
      throw LowlevelError("Native decompilation did not finish");
    std::ostringstream markup;
    architecture.print->setOutputStream(&markup);
    architecture.print->setMarkup(true);
    architecture.print->setPackedOutput(false);
    architecture.print->docFunction(function);
    std::cout << "<result><markup>" << markup.str() << "</markup><operations>";
    for (auto op = function->beginOpMain(); op != function->endOpMain(); ++op) {
      const PcodeOp *operation = op->second;
      std::cout << "<op id=\"" << std::dec << operation->getTime()
                << "\" address=\"0x" << std::hex << operation->getAddr().getOffset() << "\"/>";
    }
    std::cout << "</operations><diagnostic>" << escaped(diagnostics.str())
              << "</diagnostic></result>\n";
    return 0;
  } catch (const LowlevelError &error) {
    std::cerr << error.explain << '\n';
  } catch (const DecoderError &error) {
    std::cerr << error.explain << '\n';
  } catch (const std::exception &error) {
    std::cerr << error.what() << '\n';
  }
  return 1;
}
