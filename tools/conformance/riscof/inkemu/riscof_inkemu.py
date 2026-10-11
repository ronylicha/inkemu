"""RISCOF DUT plugin: compiles each arch test and runs it on `inkemu rvtest`."""
import os

from riscof import utils
from riscof.pluginTemplate import pluginTemplate


class inkemu(pluginTemplate):
    __model__ = "inkemu"
    __version__ = "0.1.0"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        config = kwargs.get("config")
        if config is None:
            raise SystemExit("inkemu plugin: missing [inkemu] section in config.ini")
        # configparser may lowercase option names depending on how RISCOF builds the section.
        dut_dir = config.get("PATH") or config.get("path", "")
        self.dut_exe = os.path.join(dut_dir, "inkemu")
        self.num_jobs = str(config.get("jobs", 1))
        self.pluginpath = os.path.abspath(config["pluginpath"])
        self.isa_spec = os.path.abspath(config["ispec"])
        self.platform_spec = os.path.abspath(config["pspec"])
        self.target_run = config.get("target_run", "1") == "1"

    def initialise(self, suite, work_dir, archtest_env):
        self.work_dir = work_dir
        self.suite_dir = suite
        self.compile_cmd = (
            "riscv64-unknown-elf-gcc -march={march} -mabi=ilp32 -static -mcmodel=medany "
            "-fvisibility=hidden -nostdlib -nostartfiles -g "
            f"-T {self.pluginpath}/env/link.ld -I {self.pluginpath}/env/ -I {archtest_env} "
            "{macros} -o {elf} {test}"
        )

    def build(self, isa_yaml, platform_yaml):
        ispec = utils.load_yaml(isa_yaml)["hart0"]
        if 32 not in ispec["supported_xlen"]:
            raise SystemExit("inkemu plugin: only RV32 is supported")

    def runTests(self, testList):
        make = utils.makeUtil(makefilePath=os.path.join(self.work_dir, "Makefile." + self.name[:-1]))
        make.makeCommand = "make -k -j" + self.num_jobs
        for entry in testList.values():
            elf = "dut.elf"
            sig = os.path.join(entry["work_dir"], self.name[:-1] + ".signature")
            macros = " ".join("-D" + m for m in entry["macros"])
            # Test ISA strings look like "RV32IC" or "RV32I_Zicsr"; GCC needs zicsr/zifencei spelled out.
            base = entry["isa"].lower().split("_")[0].replace("zicsr", "").replace("zifencei", "")
            march = base + "_zicsr_zifencei"
            compile_cmd = self.compile_cmd.format(march=march, macros=macros, elf=elf, test=entry["test_path"])
            run_cmd = f"{self.dut_exe} rvtest {elf} --signature {sig}" if self.target_run else "true"
            make.add_target(f"@cd {entry['work_dir']}; {compile_cmd}; {run_cmd};")
        make.execute_all(self.work_dir)
        if not self.target_run:
            raise SystemExit(0)
